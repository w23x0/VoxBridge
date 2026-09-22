//! 媒体面真机探针：把两条腿在一个进程里跑起来，用**可复跑的观察出口**报事实
//! （照 `crates/vox-overlay-linux/examples/frame_loop_probe.rs` 的先例）。
//!
//! ```bash
//! # 自测（不需要盒子）：本进程发 2 秒已知正弦、本进程收，写成 WAV 并打统计
//! cargo run -p vox-net --example media_probe -- \
//!   --listen 127.0.0.1:47100 --url ws://127.0.0.1:47100/audio \
//!   --token <43 字符> --seconds 2 --out /tmp/probe.wav
//!
//! # 只当对端：往盒子的 `/audio` 灌 2 秒正弦（盒子的 Listen 腿在收）
//! cargo run -p vox-net --example media_probe -- \
//!   --url ws://127.0.0.1:47100/audio --token <43 字符> --seconds 2
//!
//! # 只当端点：把收到的音频写成 WAV（对着盒子跑时，它听的就是你发的）
//! cargo run -p vox-net --example media_probe -- \
//!   --listen 0.0.0.0:47100 --token <43 字符> --seconds 2 --out /tmp/heard.wav
//! ```
//!
//! **v1 的监听侧不回音频**（它只发保活帧，音频是"收"的方向）。所以要证"回程音频是真的"，
//! 就在同一个进程里同时开 `--listen` 与 `--url`：写出来的 WAV 就是**发出去又收回来**的音频。
//! 探针不打印 token，也不把它写进文件。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vox_core::ports::AudioChunk;
use vox_net::media::frame;
use vox_net::media::pipe::PipeConfig;
use vox_net::media::{MediaListener, MediaOptions, MediaOut, DEFAULT_PIPE};

/// 探针只跑这一个率（与 `Settings.net.rate_hz` 的缺省一致）。
const RATE: u32 = 16_000;
/// 探针发的是 440 Hz、幅度 0.5 的正弦（幅度好认，不像噪声）。
const TONE_HZ: f32 = 440.0;
const TONE_AMPLITUDE: f32 = 0.5;

fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("参数有问题：{message}");
            usage();
            return ExitCode::from(2);
        }
    };
    if args.help {
        usage();
        return ExitCode::SUCCESS;
    }
    if args.listen.is_none() && args.url.is_none() {
        eprintln!("至少要给 `--listen` 或 `--url` 一个（两个都给就是自测）。");
        usage();
        return ExitCode::from(2);
    }
    if args.out.is_some() && args.listen.is_none() {
        eprintln!("提醒：`--out` 只对 `--listen` 有效（v1 的监听侧不回音频，回程写不出东西）。");
    }

    let pipe = PipeConfig {
        keepalive_ms: 500,
        ..PipeConfig::default()
    };

    // ① 监听侧（可选）：把听到的音频攒起来。
    let heard: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let listener = match args.listen {
        Some(addr) => match MediaListener::bind(MediaOptions {
            listen: addr,
            rate_hz: RATE,
            token: args.token.clone(),
            allowed_origins: Vec::new(),
            pipe,
        }) {
            Ok(listener) => Some(listener),
            Err(e) => {
                eprintln!("媒体面绑不上：{e}");
                return ExitCode::from(1);
            }
        },
        None => None,
    };
    let mut capture = listener.as_ref().map(|listener| {
        let mut capture = listener.capture();
        let sink = Arc::clone(&heard);
        match capture.start(
            &vox_core::ports::CaptureTarget::Net {
                pipe: DEFAULT_PIPE.to_string(),
            },
            pipe.block_ms,
            Box::new(move |chunk: AudioChunk| {
                sink.lock().expect("锁没毒").extend(chunk.samples);
            }),
        ) {
            Ok(format) => println!(
                "监听 {}：{} Hz / {} 声道",
                listener.local_addr(),
                format.sample_rate,
                format.channels
            ),
            Err(e) => {
                eprintln!("媒体面采集源起不来：{e}");
                std::process::exit(1);
            }
        }
        capture
    });

    // ② 出站侧（可选）：连上去，把 2 秒正弦按 20 ms 一拍地推。
    let out = match &args.url {
        Some(url) => match MediaOut::spawn(url.clone(), args.token.clone(), pipe) {
            Ok(out) => {
                println!("出站池已起来，对端 {url}");
                Some(out)
            }
            Err(e) => {
                eprintln!("媒体面出站池起不来：{e}");
                return ExitCode::from(1);
            }
        },
        None => None,
    };
    if let Some(out) = &out {
        let mut sink = out.sink();
        if let Err(e) = sink.open(None, RATE) {
            eprintln!("网络出口打不开：{e}");
            return ExitCode::from(1);
        }
        // 连不上就没音频可发，探针要把这件事说清楚并给非零退出码（脚本靠它判，见设计稿 §4.4 ⑤）。
        let deadline = Instant::now() + Duration::from_secs(2);
        while !out.stats().connected && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !out.stats().connected {
            match out.last_error() {
                // 链路侧原话（比如 "…HTTP error: 401 Unauthorized"）比猜原因有用。
                Some(error) => eprintln!("媒体面出站 2 秒内没连上对端：{error}"),
                None => eprintln!(
                    "媒体面出站 2 秒内没连上对端（凭据不对 / 对端不在 / 端口不对），一条音频都没发出去。"
                ),
            }
            return ExitCode::from(1);
        }
        let block = pipe.block_samples(RATE);
        let total = args.seconds as usize * RATE as usize;
        // 先灌多少块：预填（jitter_ms）再多给 4 块余量——只给预填那点会被 sleep 的抖动吃光。
        let lead = (pipe.jitter_ms / pipe.block_ms + 4) as usize;
        let tone: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                (t * TONE_HZ * std::f32::consts::TAU).sin() * TONE_AMPLITUDE
            })
            .collect();
        // 按**绝对时刻表**喂（不是每轮回 sleep）：sleep 的抖动会一格格累积成
        // "供给慢于排空"，那会平白变成 padded_ms。第 k 块的目标时刻是 lead + k 拍。
        let started = Instant::now();
        for (i, chunk) in tone.chunks(block).enumerate() {
            sink.push(chunk);
            let paced = (i + 1).saturating_sub(lead);
            if paced > 0 {
                let target = Duration::from_millis(paced as u64 * u64::from(pipe.block_ms));
                let elapsed = started.elapsed();
                if target > elapsed {
                    std::thread::sleep(target - elapsed);
                }
            }
        }
        sink.flush();
        println!(
            "发完 {} 秒正弦（{} 帧 @ {} Hz）",
            args.seconds,
            total / block,
            RATE
        );
    }

    // ③ 让监听侧把余下的排空。自己发的就按**样本数**等（多等一拍卖空之后会平白多出
    // padded_ms，少等会把尾巴截掉）；别人发的不知道总量，按 `--seconds` 等。
    let want = args.seconds as usize * RATE as usize;
    if out.is_some() {
        let deadline = Instant::now() + Duration::from_secs(3);
        while heard.lock().expect("锁没毒").len() < want && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    } else if capture.is_some() {
        std::thread::sleep(Duration::from_secs(args.seconds));
    }
    if let Some(capture) = capture.as_mut() {
        capture.stop();
    }

    // ④ 统计：两条腿各打一份（不含任何音频内容、不含 token）。
    if let Some(listener) = &listener {
        let stats = listener.stats();
        println!(
            "监听侧：frames_in={} frames_out={} bytes_in={} dropped_ms={} padded_ms={} idle_ms={} \
             seq_gaps={} bad_frames={} rate_mismatch={} connected={} reconnects={} last_frame_age_ms={}",
            stats.frames_in,
            stats.frames_out,
            stats.bytes_in,
            stats.dropped_ms,
            stats.padded_ms,
            stats.idle_ms,
            stats.seq_gaps,
            stats.bad_frames,
            stats.rate_mismatch,
            stats.connected,
            stats.reconnects,
            stats.last_frame_age_ms,
        );
    }
    if let Some(out) = &out {
        let stats = out.stats();
        println!(
            "出站侧：frames_out={} bytes_out={} dropped_ms={} connected={} reconnects={} \
             last_frame_age_ms={}",
            stats.frames_out,
            stats.bytes_out,
            stats.dropped_ms,
            stats.connected,
            stats.reconnects,
            stats.last_frame_age_ms,
        );
    }

    // ⑤ 收成 WAV，让"回程音频是真的"可以被 sox/耳朵验。
    let heard = heard.lock().expect("锁没毒").clone();
    if let Some(path) = &args.out {
        let mut pcm = Vec::with_capacity(heard.len() * 2);
        frame::push_pcm16(&heard, &mut pcm);
        match write_wav(path, RATE, &pcm) {
            Ok(()) => {
                let peak = heard.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                println!(
                    "写出 {}：{} 样本（{:.2} 秒）、峰值 {:.3}",
                    path.display(),
                    heard.len(),
                    heard.len() as f32 / RATE as f32,
                    peak
                );
            }
            Err(e) => {
                eprintln!("写 WAV 失败：{e}");
                return ExitCode::from(1);
            }
        }
    }

    if let Some(out) = &out {
        out.shutdown();
    }
    if let Some(listener) = &listener {
        listener.shutdown();
    }
    ExitCode::SUCCESS
}

/// 写一个 16 位单声道 WAV（只为探针可观察，不引新依赖）。
fn write_wav(path: &PathBuf, rate: u32, pcm: &[u8]) -> std::io::Result<()> {
    let data_len = pcm.len() as u32;
    let mut bytes = Vec::with_capacity(44 + pcm.len());
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes()); // fmt 块长
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes()); // 字节率
    bytes.extend_from_slice(&2u16.to_le_bytes()); // 块对齐
    bytes.extend_from_slice(&16u16.to_le_bytes()); // 位深
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.extend_from_slice(pcm);
    std::fs::write(path, bytes)
}

struct Args {
    listen: Option<SocketAddr>,
    url: Option<String>,
    token: String,
    seconds: u64,
    out: Option<PathBuf>,
    help: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = Args {
            listen: None,
            url: None,
            token: String::new(),
            seconds: 2,
            out: None,
            help: false,
        };
        let mut argv = std::env::args().skip(1);
        while let Some(flag) = argv.next() {
            match flag.as_str() {
                "--help" | "-h" => args.help = true,
                "--listen" => {
                    let value = argv.next().ok_or("`--listen` 后面要跟 IP:PORT")?;
                    args.listen = Some(
                        value
                            .parse()
                            .map_err(|e| format!("`--listen {value}` 不是 IP:PORT：{e}"))?,
                    );
                }
                "--url" => {
                    args.url = Some(argv.next().ok_or("`--url` 后面要跟 ws://…")?);
                }
                "--token" => {
                    args.token = argv.next().ok_or("`--token` 后面要跟凭据")?;
                }
                "--seconds" => {
                    let value = argv.next().ok_or("`--seconds` 后面要跟秒数")?;
                    args.seconds = value
                        .parse()
                        .map_err(|e| format!("`--seconds {value}` 认识不了：{e}"))?;
                }
                "--out" => {
                    args.out = Some(PathBuf::from(argv.next().ok_or("`--out` 后面要跟路径")?));
                }
                other => return Err(format!("不认识的开关 {other}")),
            }
        }
        Ok(args)
    }
}

fn usage() {
    println!(
        "媒体面探针（vox-net 的 `media` 模块）\n\
         \n用法：\n  \
         media_probe --listen <IP:PORT> --url <ws://…> --token <凭据> [--seconds N] [--out FILE]\n\
         \n开关：\n  \
         --listen <IP:PORT>  绑媒体面入站（`net_in` 的定义者）；端口 0 = 系统分配\n  \
         --url <ws://…>      连媒体面出站（`net_out`）；v1 只认 ws://（wss:// 要证书，未做）\n  \
         --token <凭据>      43 字符；两条腿都要（缺省空 = 一条都起不来，fail-closed）\n  \
         --seconds <N>       发多久正弦，缺省 2\n  \
         --out <FILE>        把 `--listen` 收到的音频写成 16 位单声道 WAV\n  \
         --help             这一屏"
    );
}
