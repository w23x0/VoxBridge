//! `CaptureSource` 的 Linux 实现：PipeWire 采集流。
//!
//! 两种目标走两条路：
//!
//! | 目标 | 怎么接 | 谁负责连线 |
//! | --- | --- | --- |
//! | `Microphone(name)` | 普通采集流，`target.object` 指定设备，不指定就走默认源 | wireplumber 的策略 |
//! | `ProcessLoopback{executable}` | 找那个程序正在放音的节点，把它的输出端口连到我们的输入端口 | **我们自己**（见下） |
//!
//! 实测结论（决定这里怎么写）：
//!
//! - `target.object` 指向**某个程序的播放流节点**时，wireplumber 会正确连过去
//!   （`pw-record --target=<node id>` 抓 pw-play 的 peak 与源一致）。所以主目标走
//!   `target.object` + 自动连接这条最省事的路。
//! - 但 `target.object` 对**采集流连 sink**（monitor）会被忽略，会偷偷连到默认源上、
//!   录出全 0 —— 这也是为什么虚拟麦的回环验证要用 `pw-link` 显式连，不能靠 target。
//!
//! **已知限制**：`target.object` 一次只认一个节点，所以同一程序有多条播放流时
//! （Chromium 每个标签页一条）只抓主目标，其余记一条 warn 日志。做过一版"主目标走
//! `target.object` + 其余用 `link-factory` 显式连进来混音"的实现，但那条路上流根本
//! 进不了 Streaming（启动必然超时），没敢留——混音要么另找办法（在 `process` 里多流
//! 汇聚），要么等真有用户需要再说。
//!
//! **故意不设 `RT_PROCESS`**：PipeWire 的 process 回调默认跑在实时线程上，而内核的
//! `on_chunk` 会加锁 + 分配 `AudioChunk`（`ports.rs` 定的接口形状），在实时线程里干这个
//! 是给自己找优先级反转。Windows 侧也一样——采集发生在自己起的普通线程上。

use std::collections::HashSet;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::pod::Pod;
use vox_core::ports::{
    AudioChunk, CaptureFormat, CaptureSource, CaptureTarget, PortError, PortResult,
};
use vox_dsp::chunk::Blocker;

use crate::probe::{self, CLASS_OUTPUT_STREAM};

/// 等流进入 Streaming 的上限（跟 Windows 侧同一个量级）。
const START_TIMEOUT: Duration = Duration::from_secs(8);
/// 我们向图请求的格式。48 kHz 是 RNNoise 的原生率，让图去转比我们转省事。
const REQUEST_RATE: u32 = 48_000;
const REQUEST_CHANNELS: u16 = 2;
const NODE_NAME: &str = "VoxBridge 采集";

/// 采集计划：`start` 时定下来，之后线程照着做。
#[derive(Debug)]
enum Plan {
    /// 麦克风（`None` = 默认源）。
    Microphone(Option<String>),
    /// 按程序抓音：主目标的节点 id，以及同一程序**没被抓**的其它流数量。
    ///
    /// 多条流混音还没做（见模块头的"已知限制"），但至少要让用户/日志知道少抓了。
    Process {
        primary: String,
        skipped: usize,
        label: String,
    },
}

/// 从流水线线程发给采集线程的命令。
enum Cmd {
    Quit,
}

struct Shared {
    stop: std::sync::atomic::AtomicBool,
    /// 启动回报通道。协商结果、启动错误都从这儿出去；谁先拿到谁负责回报。
    report: std::sync::Mutex<Option<mpsc::Sender<PortResult<CaptureFormat>>>>,
}

struct Running {
    shared: Arc<Shared>,
    cmd: pw::channel::Sender<Cmd>,
    thread: JoinHandle<()>,
}

/// 采集源。`start` 之后音频块通过回调推给内核，`stop` 保证回调不再触发。
pub struct LinuxCapture {
    running: Option<Running>,
}

impl LinuxCapture {
    pub fn new() -> Self {
        Self { running: None }
    }
}

impl Default for LinuxCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureSource for LinuxCapture {
    fn start(
        &mut self,
        target: &CaptureTarget,
        block_ms: u32,
        on_chunk: Box<dyn FnMut(AudioChunk) + Send>,
    ) -> PortResult<CaptureFormat> {
        // 重复 start 视为换目标：先把旧的收干净，否则两个流会同时往回调里灌数据。
        self.stop();

        let plan = resolve_plan(target)?;
        let (report_tx, report_rx) = mpsc::channel::<PortResult<CaptureFormat>>();
        let shared = Arc::new(Shared {
            stop: std::sync::atomic::AtomicBool::new(false),
            report: std::sync::Mutex::new(Some(report_tx)),
        });
        let (cmd_tx, cmd_rx) = pw::channel::channel::<Cmd>();

        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("vox-capture".into())
            .spawn(move || capture_thread(plan, block_ms, thread_shared, cmd_rx, on_chunk))
            .map_err(|e| PortError::new(format!("创建采集线程失败：{e}")))?;

        self.running = Some(Running {
            shared,
            cmd: cmd_tx,
            thread,
        });

        match report_rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(format)) => Ok(format),
            Ok(Err(e)) => {
                self.stop();
                Err(e)
            }
            Err(RecvTimeoutError::Timeout) => {
                self.stop();
                Err(PortError::new(format!(
                    "采集启动超时（等了 {} 秒还没就绪）",
                    START_TIMEOUT.as_secs()
                )))
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.stop();
                Err(PortError::new("采集线程意外退出"))
            }
        }
    }

    fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.shared.stop.store(true, std::sync::atomic::Ordering::Release);
            let _ = running.cmd.send(Cmd::Quit);
            // 线程退出即代表回调不会再触发——这是 trait 契约里唯一能给的保证。
            //
            // **有界等待**：`pw::channel` 的唤醒依赖 PipeWire 主循环（pipewire-rs 自己的
            // 文档里那个例子就挂在 issue #19 上），万一它没把 Quit 送进循环，无界 join 会把
            // 整个流水线卡死在 stop 上。宁可漏一个线程，也不能让用户点"停止"没反应。
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !running.thread.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if running.thread.is_finished() {
                let _ = running.thread.join();
            } else {
                tracing::warn!("采集线程没在 2 秒内退出，先放它自生自灭（不再 join）");
            }
        }
    }
}

impl Drop for LinuxCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 把内核给的 `CaptureTarget` 翻译成"抓哪些节点"。
fn resolve_plan(target: &CaptureTarget) -> PortResult<Plan> {
    match target {
        CaptureTarget::Microphone(name) => {
            if let Some(name) = name.as_deref() {
                let snapshot = probe::snapshot()?;
                let known = snapshot
                    .nodes_of_class(probe::CLASS_SOURCE)
                    .iter()
                    .chain(snapshot.nodes_of_class(probe::CLASS_SOURCE_VIRTUAL).iter())
                    .any(|node| node.name == name || node.description == name);
                if !known {
                    return Err(PortError::new(format!("找不到输入设备「{name}」")));
                }
            }
            Ok(Plan::Microphone(name.clone()))
        }
        CaptureTarget::ProcessLoopback {
            executable,
            include_tree,
        } => {
            let snapshot = probe::snapshot()?;
            let streams = snapshot.output_streams_of(executable);
            if streams.is_empty() {
                return Err(PortError::new(format!(
                    "「{executable}」现在没有在放声音（按程序抓音只抓它正在播的流）"
                )));
            }
            let mut pids: HashSet<u32> = HashSet::new();
            for stream in &streams {
                if let Some(pid) = snapshot.pid_of(stream) {
                    pids.insert(pid);
                }
            }
            if *include_tree {
                // Chromium 那种一个程序十几个进程的：子进程放的音也要抓。
                for pid in pids.clone() {
                    pids.extend(descendants(pid));
                }
            }
            let targets: Vec<u32> = snapshot
                .nodes
                .iter()
                .filter(|(_, node)| node.media_class == CLASS_OUTPUT_STREAM)
                .filter(|(_, node)| snapshot.pid_of(node).is_some_and(|pid| pids.contains(&pid)))
                .map(|(id, _)| *id)
                .collect();
            if targets.is_empty() {
                return Err(PortError::new(format!(
                    "找到了「{executable}」的音频流，但定位不到它的进程（拿不到 pid）"
                )));
            }
            let primary = targets
                .first()
                .ok_or_else(|| PortError::new("目标音频流列表是空的"))?
                .to_string();
            Ok(Plan::Process {
                primary,
                skipped: targets.len() - 1,
                label: executable.clone(),
            })
        }
    }
}

/// 某个进程的所有后代（沿 `/proc/<pid>/task/*/children` 递归）。
///
/// 只读 /proc，不需要额外依赖；拿不到就返回空表（宁可少抓，不能抓错）。
fn descendants(root: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut queue = vec![root];
    let mut seen = HashSet::new();
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        let tasks = match std::fs::read_dir(format!("/proc/{pid}/task")) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for task in tasks.flatten() {
            let children = std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
            for child in children.split_whitespace() {
                if let Ok(child) = child.parse::<u32>() {
                    if seen.insert(child) {
                        out.push(child);
                        queue.push(child);
                    }
                }
            }
        }
    }
    out
}

/// 采集线程：建流 → 连流 → （按程序抓音时）自己建链 → 跑主循环。
fn capture_thread(
    plan: Plan,
    block_ms: u32,
    shared: Arc<Shared>,
    cmd_rx: pw::channel::Receiver<Cmd>,
    on_chunk: Box<dyn FnMut(AudioChunk) + Send>,
) {
    let result = (|| -> PortResult<()> {
        pw::init();
        let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(map_err)?;
        let context = pw::context::ContextRc::new(&main_loop, None).map_err(map_err)?;
        let core = context.connect_rc(None).map_err(map_err)?;

        let loop_weak = main_loop.downgrade();
        let _attached = cmd_rx.attach(main_loop.loop_(), move |cmd| match cmd {
            Cmd::Quit => {
                if let Some(main_loop) = loop_weak.upgrade() {
                    main_loop.quit();
                }
            }
        });

        let mut props = properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Communication",
            *pw::keys::NODE_NAME => NODE_NAME,
        };
        match &plan {
            Plan::Microphone(Some(device)) => {
                props.insert("target.object", device.as_str());
            }
            Plan::Microphone(None) => {}
            Plan::Process { primary, .. } => {
                // 主目标交给 session manager 连（实测可靠）；多出来的流后面自己连。
                props.insert("target.object", primary.as_str());
            }
        }

        let stream = pw::stream::StreamBox::new(&core, NODE_NAME, props).map_err(map_err)?;

        struct UserData {
            shared: Arc<Shared>,
            negotiated: Option<CaptureFormat>,
            blocker: Option<Blocker>,
            block_ms: u32,
            on_chunk: Box<dyn FnMut(AudioChunk) + Send>,
        }

        let _listener = stream
            .add_local_listener_with_user_data(UserData {
                shared: Arc::clone(&shared),
                negotiated: None,
                blocker: None,
                block_ms,
                on_chunk,
            })
            .param_changed(|_stream, data, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let mut info = spa::param::audio::AudioInfoRaw::new();
                if info.parse(param).is_err() {
                    return;
                }
                let rate = info.rate();
                let channels = info.channels() as u16;
                if rate == 0 || channels == 0 {
                    return;
                }
                data.negotiated = Some(CaptureFormat {
                    sample_rate: rate,
                    channels,
                });
                data.blocker = Some(Blocker::new(rate, channels, data.block_ms));
                report_started(&data.shared, CaptureFormat { sample_rate: rate, channels });
            })
            .state_changed(|_stream, data, _old, new| {
                if new == pw::stream::StreamState::Streaming && data.negotiated.is_none() {
                    // 协商没回来就进 Streaming：按请求值开工，别让上层干等超时。
                    data.negotiated = Some(CaptureFormat {
                        sample_rate: REQUEST_RATE,
                        channels: REQUEST_CHANNELS,
                    });
                    data.blocker = Some(Blocker::new(REQUEST_RATE, REQUEST_CHANNELS, data.block_ms));
                    report_started(
                        &data.shared,
                        CaptureFormat {
                            sample_rate: REQUEST_RATE,
                            channels: REQUEST_CHANNELS,
                        },
                    );
                }
                if let pw::stream::StreamState::Error(message) = new {
                    report_failed(
                        &data.shared,
                        format!("采集流出错：{message}"),
                    );
                }
            })
            .process(|stream, data| {
                if data.shared.stop.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let datas = buffer.datas_mut();
                let Some(data_buf) = datas.first_mut() else {
                    return;
                };
                // **必须只看 chunk.size() 指的那一段**：`data()` 给的是整个映射缓冲，
                // 后面的字节是上一轮的残留。按整块算会把样本数放大十几倍（实测过）。
                let channels = data.negotiated.map_or(1u16, |f| f.channels).max(1) as usize;
                let (offset, size) = {
                    let chunk = data_buf.chunk();
                    (chunk.offset() as usize, chunk.size() as usize)
                };
                // 采样格式是 f32 交错，所以一帧 = channels 个 f32；stride 只是核对用。
                debug_assert!(channels > 0);
                let Some(slice) = data_buf.data() else {
                    return;
                };
                let end = (offset + size).min(slice.len());
                if end <= offset {
                    return;
                }
                let samples = as_f32_slice(&slice[offset..end]);
                // 分开借：blocker 和 on_chunk 都在 user data 里。
                let UserData {
                    blocker, on_chunk, ..
                } = data;
                if let Some(blocker) = blocker.as_mut() {
                    blocker.feed(samples, &mut **on_chunk);
                }
            })
            .register()
            .map_err(map_err)?;

        let mut audio_info = spa::param::audio::AudioInfoRaw::new();
        audio_info.set_format(spa::param::audio::AudioFormat::F32LE);
        audio_info.set_rate(REQUEST_RATE);
        audio_info.set_channels(REQUEST_CHANNELS as u32);
        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = pw::spa::sys::SPA_AUDIO_CHANNEL_FR;
        audio_info.set_position(position);

        let values = pw::spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &pw::spa::pod::Value::Object(pw::spa::pod::Object {
                type_: pw::spa::sys::SPA_TYPE_OBJECT_Format,
                id: pw::spa::sys::SPA_PARAM_EnumFormat,
                properties: audio_info.into(),
            }),
        )
        .map_err(|e| PortError::new(format!("构造音频格式失败：{e}")))?
        .0
        .into_inner();
        let pod =
            Pod::from_bytes(&values).ok_or_else(|| PortError::new("音频格式序列化结果不合法"))?;
        let mut params = [pod];

        stream
            .connect(
                spa::utils::Direction::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut params,
            )
            .map_err(map_err)?;

        // 同一程序多条流：主目标已经由 session manager 连上；剩下的暂时不抓（见模块头）。
        if let Plan::Process { skipped, label, .. } = &plan {
            if *skipped > 0 {
                tracing::warn!(
                    "「{label}」还有 {skipped} 条音频流没抓：多条流混音还没做，当前只抓主目标"
                );
            }
        }

        main_loop.run();
        Ok(())
    })();

    if let Err(e) = result {
        // 还没回报过就交给 start()（它在等这个通道）；已经开工了就只记日志。
        if !report_failed(&shared, e.message.clone()) {
            tracing::error!("采集线程退出：{e}");
        }
    }
}

/// 把协商结果回报给 `start()`。只有第一次有效（通道被取走后就没了）。
fn report_started(shared: &Shared, format: CaptureFormat) {
    if let Some(report) = shared.report.lock().ok().and_then(|mut slot| slot.take()) {
        let _ = report.send(Ok(format));
    }
}

/// 把失败回报给 `start()`。返回 `true` 表示确实发出去了（说明还没开工）。
fn report_failed(shared: &Shared, message: String) -> bool {
    match shared.report.lock().ok().and_then(|mut slot| slot.take()) {
        Some(report) => {
            let _ = report.send(Err(PortError::new(message)));
            true
        }
        None => false,
    }
}

/// PipeWire 给的是字节切片，我们按 f32 读。长度一定 4 的整数倍（stride 算过）。
fn as_f32_slice(slice: &[u8]) -> &[f32] {
    // SAFETY：f32 对齐要求 4，PipeWire 的缓冲按 4 字节对齐；长度取整到 4 的倍数。
    let len = slice.len() / std::mem::size_of::<f32>();
    unsafe { std::slice::from_raw_parts(slice.as_ptr().cast::<f32>(), len) }
}

fn map_err(err: pw::Error) -> PortError {
    PortError::new(format!("PipeWire 出错：{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_of_own_process_is_empty_or_plausible() {
        // 拿自己的 pid 试：结果里不该出现自己（避免自环），也不该 panic。
        let me = std::process::id();
        let kids = descendants(me);
        assert!(!kids.contains(&me));
    }

    #[test]
    fn unknown_microphone_is_rejected_with_a_clear_message() {
        // 不存在的设备名必须报错，而不是悄悄抓默认源。
        let err = resolve_plan(&CaptureTarget::Microphone(Some(
            "definitely-not-a-device".to_string(),
        )))
        .expect_err("不存在的设备该报错");
        assert!(err.message.contains("找不到输入设备"), "{}", err.message);
    }
}
