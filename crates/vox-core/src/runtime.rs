//! 唯一账本。
//!
//! 持有全部设置 + 全部当前状态。所有读写都过它，任何人不许自己存一份状态副本。
//! 改动后广播事件给设置界面和悬浮窗。
//!
//! 上一版踩出来的六条坑，这里逐条防住（每条都有对应测试）：
//!
//! 1. 阀门初始状态**跟着会话配置一起下发**，不靠事后补一条命令。
//! 2. 停止走**握手**，不留孤儿会话。
//! 3. 阀门命令带**单调序号**，序号**在锁内**分配，防并发乱序。
//! 4. 会话回调要**验身份**，旧会话的迟到回调直接丢弃。
//! 5. 停止时把"麦克风活跃"标志**复位**。
//! 6. 一个热键只有**一条**生效路径。
//!
//! 另外：**绝不在持锁期间调监听器**。一律"锁内改状态、攒事件 → 放锁 → 发事件"，
//! 否则监听器回头读账本就死锁了。

use parking_lot::{Mutex, RwLock};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::catalog::{self, ActivationMode};
use crate::event::{Event, Notice, Pipeline, PipelineState};
use crate::gate::{GateConfig, GateStatus};
use crate::latency::LatencySnapshot;
use crate::ports::{
    AudioApp, Clock, DeviceInfo, HotkeyBindings, HotkeyEvent, HotkeyHost, PortResult, SecretStore,
};
use crate::settings::{ModelProvider, Settings};
use crate::subtitle::{Subtitles, Track};
use crate::usage::{TurnUsage, UsageLedger};

/// 一条流水线启动时下发的完整配置。
///
/// **阀门的初始状态在这里**（`gate_active`），跟会话配置一起下发。不许启动完
/// 再补一条 set_gate 命令——那个时间窗里说的话会漏传或误传。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    pub session_id: u64,
    pub pipeline: Pipeline,
    pub provider: ModelProvider,
    pub model_name: String,
    pub api_key: String,
    pub target_language: String,
    /// `None` = 只要文字不要语音。
    pub voice: Option<String>,
    pub voice_clone_frequency: Option<u32>,
    pub gate: GateConfig,
    /// 阀门初始是开的还是关的。
    pub gate_active: bool,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// 是否把译文额外回放到系统默认播放设备，供本人测试。
    pub monitor_translation: bool,
    /// 抓哪个程序（只有听人说话用）。
    pub loopback_target: Option<crate::settings::ListenTarget>,
    /// 源语言；`None` = 服务端自动识别。只有听人说话用。
    pub source_language: Option<String>,
    pub denoise: bool,
}

/// 发给流水线的命令。
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineCommand {
    Start(Box<SessionConfig>),
    /// 停。实现方**必须**等会话真的收摊了才返回（握手），不许 fire-and-forget。
    Stop {
        session_id: u64,
    },
    /// 开关阀门。`seq` 单调递增，流水线只接受比自己见过的更大的序号。
    SetGateActive {
        session_id: u64,
        seq: u64,
        active: bool,
    },
    SetGateConfig {
        session_id: u64,
        seq: u64,
        config: GateConfig,
    },
    /// 不重连的热更新（换目标语言 / 换音色）。
    HotUpdate {
        session_id: u64,
        target_language: Option<String>,
        voice: Option<String>,
    },
    /// 即时开关译文 TTS，并同步切换本地播放设备。
    SetTranslationAudio {
        session_id: u64,
        voice: Option<String>,
        output_device: Option<String>,
    },
    /// 即时开关“对外说话”的本地译音回听，不重连云端会话。
    SetMonitorTranslation {
        session_id: u64,
        enabled: bool,
    },
}

/// 流水线的控制面。由外壳注入实现。
pub trait PipelineControl: Send + Sync {
    fn apply(&self, cmd: PipelineCommand) -> PortResult<()>;
}

/// 一条流水线在账本里的记录。
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineStatus {
    pub state: PipelineState,
    /// 当前会话号。回调拿着旧号来就丢弃。
    pub session_id: u64,
    pub gate: Option<GateStatus>,
    pub last_error: Option<String>,
    pub latency: LatencySnapshot,
}

impl Default for PipelineStatus {
    fn default() -> Self {
        Self {
            state: PipelineState::Idle,
            session_id: 0,
            gate: None,
            last_error: None,
            latency: LatencySnapshot::default(),
        }
    }
}

/// 给 UI 的完整快照。UI 只认这一个结构，不去别处捞状态。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Snapshot {
    pub settings: Settings,
    pub speak: PipelineSnapshot,
    pub listen: PipelineSnapshot,
    /// 麦克风开着没（跟着热键走）。
    pub mic_active: bool,
    pub usage: UsageLedger,
    pub api_key_configured: bool,
    pub api_keys_configured: BTreeMap<ModelProvider, bool>,
    pub devices: DeviceSnapshot,
    /// 两条流水线都开了，提醒戴耳机。
    pub headphones_advised: bool,
    pub notices: Vec<Notice>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PipelineSnapshot {
    pub state: PipelineState,
    pub state_label: &'static str,
    pub gate_rms: f32,
    pub gate_open: bool,
    pub last_error: Option<String>,
    pub latency: LatencySnapshot,
}

impl PipelineSnapshot {
    fn of(status: &PipelineStatus) -> Self {
        Self {
            state: status.state,
            state_label: status.state.label(),
            gate_rms: status.gate.map_or(0.0, |g| g.rms),
            gate_open: status.gate.is_some_and(|g| g.active),
            last_error: status.last_error.clone(),
            latency: status.latency.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DeviceSnapshot {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    pub audio_apps: Vec<AudioApp>,
    pub virtual_cable_installed: bool,
}

/// 锁内的全部可变状态。
struct State {
    settings: Settings,
    pipelines: BTreeMap<Pipeline, PipelineStatus>,
    subtitles: Subtitles,
    usage: UsageLedger,
    api_keys: BTreeMap<ModelProvider, String>,
    devices: DeviceSnapshot,
    /// 热键推断出来的"正在说话"。
    mic_active: bool,
    notices: Vec<Notice>,
}

impl State {
    fn pipeline(&self, p: Pipeline) -> &PipelineStatus {
        self.pipelines.get(&p).expect("两条流水线都在初始化时建好")
    }

    fn pipeline_mut(&mut self, p: Pipeline) -> &mut PipelineStatus {
        self.pipelines
            .get_mut(&p)
            .expect("两条流水线都在初始化时建好")
    }
}

/// 用 `Arc` 而不是 `Box`：发事件前先把监听器列表快照出来，这样监听器回调里再注册
/// 监听器也不会把 `listeners` 锁锁死。
pub type Listener = Arc<dyn Fn(&Event) + Send + Sync>;

/// 账本。克隆的是 `Arc`，全进程共用一份状态。
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}

struct Inner {
    state: RwLock<State>,
    listeners: Mutex<Vec<Listener>>,
    /// 阀门命令的单调序号。**在锁内分配**（见 `alloc_seq` 的调用点）。
    gate_seq: AtomicU64,
    /// 会话号发号器。
    session_seq: AtomicU64,
    clock: Arc<dyn Clock>,
    secrets: Mutex<Option<Arc<dyn SecretStore>>>,
    control: Mutex<Option<Arc<dyn PipelineControl>>>,
    hotkeys: Mutex<Option<Arc<dyn HotkeyHost>>>,
    /// 最多留多少条提示。
    notice_cap: usize,
}

impl Runtime {
    pub fn new(settings: Settings, clock: Arc<dyn Clock>) -> Self {
        let mut settings = settings;
        settings.normalize();
        let mut pipelines = BTreeMap::new();
        for p in [Pipeline::Speak, Pipeline::Listen] {
            pipelines.insert(p, PipelineStatus::default());
        }
        let subtitles = Subtitles::new(crate::subtitle::SubtitleTiming {
            char_ttl_ms: settings.subtitle.char_ttl_ms,
            char_fade_ms: settings.subtitle.char_fade_ms,
            dim_zeros: settings.subtitle.dim_zeros,
            dim_alpha: settings.subtitle.dim_alpha,
        });
        Self {
            inner: Arc::new(Inner {
                state: RwLock::new(State {
                    settings,
                    pipelines,
                    subtitles,
                    usage: UsageLedger::default(),
                    api_keys: BTreeMap::new(),
                    devices: DeviceSnapshot::default(),
                    mic_active: false,
                    notices: Vec::new(),
                }),
                listeners: Mutex::new(Vec::new()),
                gate_seq: AtomicU64::new(0),
                session_seq: AtomicU64::new(0),
                clock,
                secrets: Mutex::new(None),
                control: Mutex::new(None),
                hotkeys: Mutex::new(None),
                notice_cap: 50,
            }),
        }
    }

    // -- 装配 ---------------------------------------------------------------

    /// 注入流水线控制面（外壳启动时调一次）。
    pub fn set_control(&self, control: Arc<dyn PipelineControl>) {
        *self.inner.control.lock() = Some(control);
    }

    pub fn set_hotkey_host(&self, host: Arc<dyn HotkeyHost>) {
        *self.inner.hotkeys.lock() = Some(host);
        self.rebind_hotkeys();
    }

    /// 注入密钥仓库，顺手把存着的密钥读进来。
    pub fn set_secret_store(&self, store: Arc<dyn SecretStore>) {
        let loaded = ModelProvider::ALL
            .into_iter()
            .map(|provider| (provider, store.load_api_key_for(provider)))
            .collect::<Vec<_>>();
        *self.inner.secrets.lock() = Some(store);
        let mut keys = Vec::new();
        let mut errors = Vec::new();
        for (provider, result) in loaded {
            match result {
                Ok(Some(key)) if !key.trim().is_empty() => {
                    keys.push((provider, key.trim().to_string()));
                }
                Ok(_) => {}
                Err(err) => errors.push(err),
            }
        }
        {
            let mut state = self.inner.state.write();
            for (provider, key) in keys {
                state.api_keys.insert(provider, key);
            }
        }
        for err in errors {
            self.notify(Notice::warning(format!("读取密钥失败：{err}")));
        }
    }

    pub fn add_listener(&self, listener: Listener) {
        self.inner.listeners.lock().push(listener);
    }

    // -- 事件 ---------------------------------------------------------------

    /// 发事件。**调用前必须已经放开状态锁**（监听器很可能回头读快照）。
    fn emit(&self, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        // 先快照监听器列表再放锁，免得监听器里注册新监听器时自锁。
        let listeners: Vec<Listener> = self.inner.listeners.lock().clone();
        for event in &events {
            for listener in &listeners {
                listener(event);
            }
        }
    }

    // -- 读 -----------------------------------------------------------------

    pub fn snapshot(&self) -> Snapshot {
        let s = self.inner.state.read();
        let speak_running = s.pipeline(Pipeline::Speak).state.is_running();
        let listen_running = s.pipeline(Pipeline::Listen).state.is_running();
        let api_keys_configured = ModelProvider::ALL
            .into_iter()
            .map(|provider| {
                (
                    provider,
                    s.api_keys.get(&provider).is_some_and(|key| !key.is_empty()),
                )
            })
            .collect();
        Snapshot {
            settings: s.settings.clone(),
            speak: PipelineSnapshot::of(s.pipeline(Pipeline::Speak)),
            listen: PipelineSnapshot::of(s.pipeline(Pipeline::Listen)),
            mic_active: s.mic_active,
            usage: s.usage.clone(),
            api_key_configured: s
                .api_keys
                .get(&s.settings.speak.provider)
                .or_else(|| s.api_keys.get(&s.settings.listen.provider))
                .is_some_and(|k| !k.is_empty()),
            api_keys_configured,
            devices: s.devices.clone(),
            headphones_advised: speak_running && listen_running,
            notices: s.notices.clone(),
        }
    }

    pub fn settings(&self) -> Settings {
        self.inner.state.read().settings.clone()
    }

    pub fn pipeline_state(&self, pipeline: Pipeline) -> PipelineState {
        self.inner.state.read().pipeline(pipeline).state
    }

    pub fn usage(&self) -> UsageLedger {
        self.inner.state.read().usage.clone()
    }

    pub fn mic_active(&self) -> bool {
        self.inner.state.read().mic_active
    }

    /// 字幕帧线程的无分配快路径；避免每 33 ms 为一个布尔值克隆整份 Settings。
    pub fn subtitle_visible(&self) -> bool {
        self.inner.state.read().settings.subtitle.visible
    }

    /// 单调毫秒时钟。流水线要打事件 id、算节流，用的得是同一个时钟。
    pub fn now_ms(&self) -> u64 {
        self.inner.clock.now_ms()
    }

    /// 当前该画的字幕帧。
    pub fn subtitle_frame(&self) -> crate::ports::SubtitleFrame {
        let s = self.inner.state.read();
        let now = self.inner.clock.now_ms();
        let mut lines = Vec::with_capacity(2);
        for (track, color) in [
            (Track::Listen, &s.settings.subtitle.listen_color),
            (Track::Speak, &s.settings.subtitle.speak_color),
        ] {
            let chars = s.subtitles.track(track).render(now);
            if !chars.is_empty() {
                lines.push(crate::ports::SubtitleLine {
                    track,
                    chars,
                    color: color.clone(),
                });
            }
        }
        crate::ports::SubtitleFrame { lines }
    }

    // -- 密钥 ---------------------------------------------------------------

    pub fn set_api_key(&self, key: &str) {
        let provider = self.inner.state.read().settings.speak.provider;
        self.set_api_key_for(provider, key);
    }

    pub fn set_api_key_for(&self, provider: ModelProvider, key: &str) {
        let key = key.trim().to_string();
        {
            let mut s = self.inner.state.write();
            if key.is_empty() {
                s.api_keys.remove(&provider);
            } else {
                s.api_keys.insert(provider, key.clone());
            }
        }
        let store = self.inner.secrets.lock().clone();
        if let Some(store) = store {
            let result = if key.is_empty() {
                store.clear_api_key_for(provider)
            } else {
                store.store_api_key_for(provider, &key)
            };
            if let Err(err) = result {
                self.notify(Notice::warning(format!("密钥保存失败：{err}")));
            }
        }
        self.emit(vec![Event::SettingsChanged {
            settings: Box::new(self.settings()),
        }]);
    }

    pub fn api_key(&self) -> Option<String> {
        let s = self.inner.state.read();
        s.api_keys.get(&s.settings.speak.provider).cloned()
    }

    // -- 设置 ---------------------------------------------------------------

    /// 改设置。`edit` 在锁内跑；改完自动 normalize，该热更新的热更新，
    /// 该重启的提示重启。
    pub fn update_settings<F>(&self, edit: F) -> Settings
    where
        F: FnOnce(&mut Settings),
    {
        let mut events = Vec::new();
        let mut commands = Vec::new();
        let settings;
        {
            let mut s = self.inner.state.write();
            let old = s.settings.clone();
            edit(&mut s.settings);
            s.settings.normalize();
            let new = s.settings.clone();
            if new == old {
                return new;
            }

            // 字幕时序变了立刻生效。
            if new.subtitle.char_ttl_ms != old.subtitle.char_ttl_ms
                || new.subtitle.char_fade_ms != old.subtitle.char_fade_ms
                || new.subtitle.dim_zeros != old.subtitle.dim_zeros
                || new.subtitle.dim_alpha != old.subtitle.dim_alpha
            {
                s.subtitles.set_timing(crate::subtitle::SubtitleTiming {
                    char_ttl_ms: new.subtitle.char_ttl_ms,
                    char_fade_ms: new.subtitle.char_fade_ms,
                    dim_zeros: new.subtitle.dim_zeros,
                    dim_alpha: new.subtitle.dim_alpha,
                });
            }

            // 关掉显示时立刻清空该轨，不能让关闭前的半句继续挂在悬浮窗上。
            if old.speak.show_translation && !new.speak.show_translation {
                let track = Pipeline::Speak.track();
                s.subtitles.track_mut(track).clear();
                events.push(Event::SubtitleCleared { track });
            }
            if old.listen.show_translation && !new.listen.show_translation {
                let track = Pipeline::Listen.track();
                s.subtitles.track_mut(track).clear();
                events.push(Event::SubtitleCleared { track });
            }

            // 切激活方式：一律回到"未开麦"，避免旧状态残留导致误上传。（坑 5）
            if new.speak.activation_mode != old.speak.activation_mode && s.mic_active {
                s.mic_active = false;
                events.push(Event::MicActive { active: false });
            }

            let speak = s.pipeline(Pipeline::Speak);
            let speak_live = speak.state.is_running();
            let speak_session = speak.session_id;

            if speak_live {
                // 换服务商/模型必须重连，语言/音色可以热更新。
                if new.speak.provider != old.speak.provider
                    || new.speak.model_name != old.speak.model_name
                    || (!catalog::supports_hot_update_language(new.speak.provider)
                        && new.speak.target_language != old.speak.target_language)
                {
                    events.push(Event::Notice {
                        notice: Notice::info("服务商/模型设置已保存，重启「对外说话」后生效")
                            .on(Pipeline::Speak),
                    });
                } else {
                    let target_language = (new.speak.target_language != old.speak.target_language)
                        .then(|| new.speak.target_language.clone());
                    if target_language.is_some() {
                        commands.push(PipelineCommand::HotUpdate {
                            session_id: speak_session,
                            target_language,
                            voice: None,
                        });
                    }
                }

                if new.speak.speak_translation != old.speak.speak_translation
                    || new.speak.voice != old.speak.voice
                    || new.speak.output_device != old.speak.output_device
                {
                    commands.push(PipelineCommand::SetTranslationAudio {
                        session_id: speak_session,
                        voice: new.speak.speak_translation.then(|| new.speak.voice.clone()),
                        output_device: new.speak.output_device.clone(),
                    });
                }

                if new.speak.monitor_translation != old.speak.monitor_translation {
                    commands.push(PipelineCommand::SetMonitorTranslation {
                        session_id: speak_session,
                        enabled: new.speak.monitor_translation,
                    });
                }

                // 阀门参数变了：序号在锁内分配。（坑 3）
                let old_gate = Self::gate_for(&old);
                let new_gate = Self::gate_for(&new);
                if new_gate != old_gate {
                    commands.push(PipelineCommand::SetGateConfig {
                        session_id: speak_session,
                        seq: self.alloc_seq_locked(),
                        config: new_gate,
                    });
                }
            }

            let listen = s.pipeline(Pipeline::Listen);
            let listen_live = listen.state.is_running();
            let listen_session = listen.session_id;
            if listen_live
                && (new.listen.provider != old.listen.provider
                    || new.listen.model_name != old.listen.model_name)
            {
                events.push(Event::Notice {
                    notice: Notice::info("模型设置已保存，重启「听人说话」后生效")
                        .on(Pipeline::Listen),
                });
            }
            if listen_live
                && (new.listen.speak_translation != old.listen.speak_translation
                    || new.listen.voice != old.listen.voice
                    || new.listen.output_device != old.listen.output_device)
            {
                commands.push(PipelineCommand::SetTranslationAudio {
                    session_id: listen_session,
                    voice: new
                        .listen
                        .speak_translation
                        .then(|| new.listen.voice.clone()),
                    output_device: new.listen.output_device.clone(),
                });
            }

            settings = new;
        }

        events.push(Event::SettingsChanged {
            settings: Box::new(settings.clone()),
        });
        self.dispatch(commands);
        self.emit(events);
        self.rebind_hotkeys();
        settings
    }

    /// 该用哪个阀门：`Hold` 用手动门；`Toggle` 用可调阈值的电平门。
    fn gate_for(settings: &Settings) -> GateConfig {
        match settings.speak.activation_mode {
            ActivationMode::Hold => GateConfig::MANUAL,
            ActivationMode::Toggle => GateConfig::level(settings.speak.gate_threshold),
        }
    }

    /// **必须在持写锁时调用**。序号在锁内分配才能保证下发顺序 = 生效顺序。（坑 3）
    fn alloc_seq_locked(&self) -> u64 {
        self.inner.gate_seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    // -- 流水线开关 ---------------------------------------------------------

    /// 开一条流水线。已经在跑就什么也不做。
    pub fn start(&self, pipeline: Pipeline) {
        let mut events = Vec::new();
        let command;
        {
            let mut s = self.inner.state.write();
            if s.pipeline(pipeline).state.is_running() {
                return;
            }
            let provider = match pipeline {
                Pipeline::Speak => s.settings.speak.provider,
                Pipeline::Listen => s.settings.listen.provider,
            };
            let Some(api_key) = s.api_keys.get(&provider).cloned().filter(|k| !k.is_empty()) else {
                drop(s);
                self.notify(Notice::error("请先配置 API 密钥").on(pipeline));
                return;
            };
            if pipeline == Pipeline::Listen && s.settings.listen.target.is_none() {
                drop(s);
                self.notify(Notice::error("请先选择监听程序").on(Pipeline::Listen));
                return;
            }

            let session_id = self.inner.session_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let status = s.pipeline_mut(pipeline);
            status.session_id = session_id;
            status.state = PipelineState::Starting;
            status.last_error = None;
            status.gate = None;

            // 阀门初始状态跟着配置一起下发。（坑 1）
            let config =
                Self::session_config(&s.settings, pipeline, session_id, api_key, s.mic_active);
            command = PipelineCommand::Start(Box::new(config));
            events.push(Event::PipelineState {
                pipeline,
                state: PipelineState::Starting,
            });
        }
        self.dispatch(vec![command]);
        self.emit(events);
    }

    fn session_config(
        settings: &Settings,
        pipeline: Pipeline,
        session_id: u64,
        api_key: String,
        mic_active: bool,
    ) -> SessionConfig {
        match pipeline {
            Pipeline::Speak => SessionConfig {
                session_id,
                pipeline,
                provider: settings.speak.provider,
                model_name: settings.speak.model_name.clone(),
                api_key,
                target_language: settings.speak.target_language.clone(),
                voice: settings
                    .speak
                    .speak_translation
                    .then(|| settings.speak.voice.clone()),
                voice_clone_frequency: settings.speak.voice_clone_frequency,
                gate: Self::gate_for(settings),
                // 两种激活方式都以账本里的开麦标志为准：Hold 是"按下才为真"，
                // Toggle 是"上次切到哪就是哪"，账本里存的就是答案。
                gate_active: mic_active,
                input_device: settings.speak.input_device.clone(),
                output_device: settings.speak.output_device.clone(),
                monitor_translation: settings.speak.monitor_translation,
                loopback_target: None,
                denoise: settings.speak.denoise,
                source_language: None,
            },
            Pipeline::Listen => SessionConfig {
                session_id,
                pipeline,
                provider: settings.listen.provider,
                model_name: settings.listen.model_name.clone(),
                api_key,
                target_language: catalog::LISTEN_TARGET_LANGUAGE.to_string(),
                voice: settings
                    .listen
                    .speak_translation
                    .then(|| settings.listen.voice.clone()),
                voice_clone_frequency: None,
                // 听人说话不设电平门：threshold <= 0 无条件放行，原样上传。
                // 理由见 settings.rs —— 环回抓的是对方程序的数字源，底噪是假信号，
                // 电平门会被底噪骗"正在说话"而误开，白白烧 token；干脆全放行，
                // 省 token 靠服务端的 VAD / 我们自己的 gate 之外的手段。
                gate: GateConfig::level(0.0),
                // 电平门全部放行，初始就开着（等于没有门）。
                gate_active: true,
                input_device: None,
                output_device: settings.listen.output_device.clone(),
                monitor_translation: false,
                loopback_target: settings.listen.target.clone(),
                // 数字音源本来就干净，不降噪。
                denoise: false,
                // 源语言；None = 服务端自动识别。
                source_language: settings.listen.source_language.clone(),
            },
        }
    }

    /// 停一条流水线。停止走握手（`PipelineCommand::Stop` 的实现方要等真收摊）。（坑 2）
    pub fn stop(&self, pipeline: Pipeline) {
        let mut events = Vec::new();
        let mut commands = Vec::new();
        {
            let mut s = self.inner.state.write();
            let status = s.pipeline_mut(pipeline);
            if !status.state.is_running() && status.state != PipelineState::Failed {
                return;
            }
            let session_id = status.session_id;
            status.state = PipelineState::Idle;
            status.gate = None;
            // 会话号作废：迟到的回调会因为号不对被丢掉。（坑 4）
            status.session_id = 0;
            commands.push(PipelineCommand::Stop { session_id });
            events.push(Event::PipelineState {
                pipeline,
                state: PipelineState::Idle,
            });

            // 停对外说话要复位麦克风状态。（坑 5）
            if pipeline == Pipeline::Speak && s.mic_active {
                s.mic_active = false;
                events.push(Event::MicActive { active: false });
            }

            let track = pipeline.track();
            s.subtitles.track_mut(track).clear();
            events.push(Event::SubtitleCleared { track });
        }
        self.dispatch(commands);
        self.emit(events);
    }

    pub fn toggle(&self, pipeline: Pipeline) {
        if self.pipeline_state(pipeline).is_running() {
            self.stop(pipeline);
        } else {
            self.start(pipeline);
        }
    }

    // -- 热键 ---------------------------------------------------------------

    /// 把当前该监听的热键推给外壳。一个键**只有这一条**生效路径。（坑 6）
    pub fn rebind_hotkeys(&self) {
        let host = self.inner.hotkeys.lock().clone();
        let Some(host) = host else { return };
        let bindings = {
            let s = self.inner.state.read();
            HotkeyBindings {
                speak: Some(s.settings.speak.hotkey.clone()),
                listen: s.settings.listen.hotkey.clone(),
            }
        };
        if let Err(err) = host.rebind(bindings) {
            self.notify(Notice::warning(format!("热键注册失败：{err}")));
        }
    }

    /// 外壳把热键事件丢进来，这里是唯一的处理入口。
    pub fn on_hotkey(&self, event: HotkeyEvent) {
        match event {
            HotkeyEvent::SpeakPressed => match self.settings().speak.activation_mode {
                ActivationMode::Toggle => self.set_mic_active(!self.mic_active()),
                ActivationMode::Hold => self.set_mic_active(true),
            },
            HotkeyEvent::SpeakReleased => {
                if self.settings().speak.activation_mode == ActivationMode::Hold {
                    self.set_mic_active(false);
                }
            }
            HotkeyEvent::ListenPressed => self.toggle(Pipeline::Listen),
        }
    }

    /// 开麦 / 闭麦。阀门命令的序号在锁内分配。（坑 3）
    pub fn set_mic_active(&self, active: bool) {
        let mut events = Vec::new();
        let mut commands = Vec::new();
        let mut autostart = false;
        {
            let mut s = self.inner.state.write();
            if s.mic_active == active {
                return;
            }
            s.mic_active = active;
            events.push(Event::MicActive { active });

            let status = s.pipeline(Pipeline::Speak);
            if status.state.is_running() {
                commands.push(PipelineCommand::SetGateActive {
                    session_id: status.session_id,
                    seq: self.alloc_seq_locked(),
                    active,
                });
            } else if active {
                // 还没开流水线就按了热键：顺手把它开起来，省一步。
                autostart = true;
            }
        }
        self.dispatch(commands);
        self.emit(events);
        if autostart {
            self.start(Pipeline::Speak);
        }
    }

    // -- 外壳回调（全部验会话号）--------------------------------------------

    /// 流水线报告自己换了阶段。旧会话的迟到回调直接丢弃。（坑 4）
    pub fn on_pipeline_state(&self, pipeline: Pipeline, session_id: u64, state: PipelineState) {
        let mut events = Vec::new();
        {
            let mut s = self.inner.state.write();
            let status = s.pipeline_mut(pipeline);
            if status.session_id != session_id || session_id == 0 {
                return;
            }
            if status.state == state {
                return;
            }
            status.state = state;
            if state != PipelineState::Failed {
                status.last_error = None;
            }
            events.push(Event::PipelineState { pipeline, state });
        }
        self.emit(events);
    }

    /// 流水线挂了。
    pub fn on_pipeline_failed(&self, pipeline: Pipeline, session_id: u64, error: String) {
        let mut events = Vec::new();
        {
            let mut s = self.inner.state.write();
            let status = s.pipeline_mut(pipeline);
            if status.session_id != session_id || session_id == 0 {
                return;
            }
            status.state = PipelineState::Failed;
            status.last_error = Some(error.clone());
            status.gate = None;
            status.session_id = 0;
            events.push(Event::PipelineState {
                pipeline,
                state: PipelineState::Failed,
            });

            if pipeline == Pipeline::Speak && s.mic_active {
                s.mic_active = false;
                events.push(Event::MicActive { active: false });
            }
        }
        self.notify(Notice::error(error).on(pipeline));
        self.emit(events);
    }

    /// 阀门状态更新。高频，只更新账本里的采样值。
    pub fn on_gate_status(&self, pipeline: Pipeline, session_id: u64, status: GateStatus) {
        {
            let mut s = self.inner.state.write();
            let slot = s.pipeline_mut(pipeline);
            if slot.session_id != session_id || session_id == 0 {
                return;
            }
            slot.gate = Some(status);
        }
        self.emit(vec![Event::GateStatus { pipeline, status }]);
    }

    /// 延迟统计更新。旧会话的迟到数据与其他流水线回调一样直接丢弃。
    pub fn on_latency(&self, pipeline: Pipeline, session_id: u64, latency: LatencySnapshot) {
        {
            let mut s = self.inner.state.write();
            let slot = s.pipeline_mut(pipeline);
            if slot.session_id != session_id || session_id == 0 {
                return;
            }
            if slot.latency == latency {
                return;
            }
            slot.latency = latency.clone();
        }
        self.emit(vec![Event::LatencyChanged {
            pipeline,
            latency: Box::new(latency),
        }]);
    }

    /// 模型吐字幕。`done` = 这一段说完了；`replace` = 服务端整句重写，字幕要整行替换。
    pub fn on_subtitle_delta(
        &self,
        pipeline: Pipeline,
        session_id: u64,
        text: &str,
        done: bool,
        replace: bool,
    ) {
        let track = pipeline.track();
        {
            let mut s = self.inner.state.write();
            if s.pipeline(pipeline).session_id != session_id || session_id == 0 {
                return;
            }
            let show_translation = match pipeline {
                Pipeline::Speak => s.settings.speak.show_translation,
                Pipeline::Listen => s.settings.listen.show_translation,
            };
            if !show_translation {
                return;
            }
            let now = self.inner.clock.now_ms();
            let slot = s.subtitles.track_mut(track);
            if !text.is_empty() {
                if replace {
                    slot.replace_text(text, now);
                } else {
                    slot.push_text(text, now);
                }
            }
            if done {
                slot.finish_segment();
            }
        }
        self.emit(vec![Event::SubtitleDelta {
            track,
            text: text.to_string(),
            done,
            replace,
        }]);
    }

    pub fn clear_subtitles(&self, track: Track) {
        self.inner.state.write().subtitles.track_mut(track).clear();
        self.emit(vec![Event::SubtitleCleared { track }]);
    }

    /// 服务端回报识别到的源文语种（自动识别下才有）。只通知前端显示小字，
    /// 不落进任何内部状态——这不是行为开关，是给用户看的提示。
    pub fn on_source_detected(&self, pipeline: Pipeline, language: String) {
        let track = pipeline.track();
        self.emit(vec![Event::SourceDetected { track, language }]);
    }

    /// 掐掉过期的字幕字。悬浮窗每帧调一次。
    pub fn prune_subtitles(&self) {
        let now = self.inner.clock.now_ms();
        self.inner.state.write().subtitles.prune(now);
    }

    // -- 用量 ---------------------------------------------------------------

    /// 记一轮 token 用量。只累加模型报回来的数，不算钱。
    pub fn record_usage(&self, model: &str, usage: &TurnUsage) {
        if usage.is_zero() {
            return;
        }
        let usage_snapshot = {
            let mut s = self.inner.state.write();
            let stamp = self.inner.clock.stamp();
            s.usage.record(model, usage, stamp);
            s.usage.clone()
        };
        self.emit(vec![Event::UsageChanged {
            usage: Box::new(usage_snapshot),
        }]);
    }

    /// 从磁盘装入已有的用量账本（启动时调）。
    pub fn load_usage(&self, ledger: UsageLedger) {
        self.inner.state.write().usage = ledger;
        let usage = self.usage();
        self.emit(vec![Event::UsageChanged {
            usage: Box::new(usage),
        }]);
    }

    pub fn reset_usage(&self) {
        self.inner.state.write().usage.reset();
        let usage = self.usage();
        self.emit(vec![Event::UsageChanged {
            usage: Box::new(usage),
        }]);
    }

    // -- 设备目录 -----------------------------------------------------------

    pub fn set_devices(&self, devices: DeviceSnapshot) {
        {
            let mut s = self.inner.state.write();
            if s.devices.inputs == devices.inputs
                && s.devices.outputs == devices.outputs
                && s.devices.audio_apps == devices.audio_apps
                && s.devices.virtual_cable_installed == devices.virtual_cable_installed
            {
                return;
            }
            s.devices = devices;
        }
        self.emit(vec![Event::DevicesChanged]);
    }

    /// 无条件发一声 `DevicesChanged`，绕过 `set_devices` 的去重。
    ///
    /// 16 声道端点的启停不会改动 `outputs` 列表（禁用后 Core Audio 仍可能把旧
    /// MMDevice 报成 ACTIVE），`set_devices` 会静默吞掉这次变化，前端就永远等不
    /// 到新快照。命令在 toggle 之后调用本方法强制通知一次。
    pub fn touch_devices(&self) {
        self.emit(vec![Event::DevicesChanged]);
    }

    // -- 提示 ---------------------------------------------------------------

    pub fn notify(&self, notice: Notice) {
        {
            let mut s = self.inner.state.write();
            s.notices.push(notice.clone());
            let cap = self.inner.notice_cap;
            if s.notices.len() > cap {
                let cut = s.notices.len() - cap;
                s.notices.drain(..cut);
            }
        }
        self.emit(vec![Event::Notice { notice }]);
    }

    pub fn clear_notices(&self) {
        self.inner.state.write().notices.clear();
    }

    // -- 命令下发 -----------------------------------------------------------

    /// 按顺序下发命令。**必须在放开状态锁之后调用**。
    fn dispatch(&self, commands: Vec<PipelineCommand>) {
        if commands.is_empty() {
            return;
        }
        let control = self.inner.control.lock().clone();
        let Some(control) = control else { return };
        for cmd in commands {
            if let Err(err) = control.apply(cmd) {
                tracing::warn!(error = %err, "流水线命令失败");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::{GateKind, GateState};
    use crate::usage::Stamp;

    struct TestClock;

    impl Clock for TestClock {
        fn now_ms(&self) -> u64 {
            0
        }
        fn stamp(&self) -> Stamp {
            Stamp {
                unix_secs: 0,
                year: 2026,
                month: 8,
                day: 5,
            }
        }
    }

    #[derive(Default)]
    struct Recorder {
        commands: Mutex<Vec<PipelineCommand>>,
    }

    impl PipelineControl for Recorder {
        fn apply(&self, cmd: PipelineCommand) -> PortResult<()> {
            self.commands.lock().push(cmd);
            Ok(())
        }
    }

    impl Recorder {
        fn drain(&self) -> Vec<PipelineCommand> {
            std::mem::take(&mut *self.commands.lock())
        }
    }

    /// 建一个已填密钥、已注入控制面的账本。
    fn fixture() -> (Runtime, Arc<Recorder>) {
        let rt = Runtime::new(Settings::default(), Arc::new(TestClock));
        let rec = Arc::new(Recorder::default());
        rt.set_control(rec.clone());
        rt.set_api_key("sk-test");
        rec.drain();
        (rt, rec)
    }

    /// 让某条流水线进入"已就绪"，返回它的会话号。
    fn bring_up(rt: &Runtime, rec: &Recorder, pipeline: Pipeline) -> u64 {
        rt.start(pipeline);
        let commands = rec.drain();
        let session_id = commands
            .iter()
            .find_map(|c| match c {
                PipelineCommand::Start(cfg) if cfg.pipeline == pipeline => Some(cfg.session_id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("start 应该下发 Start，实际是 {commands:?}"));
        rt.on_pipeline_state(pipeline, session_id, PipelineState::Ready);
        session_id
    }

    fn gate_status(active: bool) -> GateStatus {
        GateStatus {
            kind: GateKind::Level,
            state: if active {
                GateState::Speech
            } else {
                GateState::Silence
            },
            rms: 0.05,
            active,
            ended: false,
        }
    }

    // --- 坑 1：阀门初始状态跟配置一起下发 ---------------------------------

    #[test]
    fn pit1_start_carries_gate_state_in_config() {
        let (rt, rec) = fixture();
        // 先按热键开麦（此时流水线还没起，会顺带自启）。
        rt.on_hotkey(HotkeyEvent::SpeakPressed);
        let commands = rec.drain();
        // 只应有一条 Start，不该有事后补的 SetGateActive。
        assert_eq!(commands.len(), 1, "启动阶段不许再补阀门命令：{commands:?}");
        match &commands[0] {
            PipelineCommand::Start(cfg) => {
                assert!(cfg.gate_active, "开着麦启动，配置里就得是开的");
                assert_eq!(cfg.pipeline, Pipeline::Speak);
            }
            other => panic!("应该是 Start，实际 {other:?}"),
        }
    }

    #[test]
    fn pit1_listen_config_opens_unconditional_gate() {
        let (rt, rec) = fixture();
        rt.update_settings(|s| {
            s.listen.target = Some(crate::settings::ListenTarget {
                executable: "vrchat.exe".into(),
                display_name: "VRChat".into(),
                include_process_tree: true,
            });
        });
        rec.drain();
        rt.start(Pipeline::Listen);
        match rec.drain().into_iter().next() {
            Some(PipelineCommand::Start(cfg)) => {
                assert!(cfg.gate_active, "听人说话起来就该等着声音");
                assert!(!cfg.denoise, "数字音源不降噪");
                // 电频门阈值 0：无条件放行，环回的数字源不会被假底噪骗开。
                assert_eq!(cfg.gate.kind, GateKind::Level);
                assert_eq!(cfg.gate.threshold, 0.0);
            }
            other => panic!("应该是 Start，实际 {other:?}"),
        }
    }

    // --- 坑 2：停止不留孤儿会话 -------------------------------------------

    #[test]
    fn pit2_stop_sends_handshake_with_the_live_session_id() {
        let (rt, rec) = fixture();
        let session_id = bring_up(&rt, &rec, Pipeline::Speak);
        rt.stop(Pipeline::Speak);
        let commands = rec.drain();
        assert!(
            commands.contains(&PipelineCommand::Stop { session_id }),
            "停止必须带上会话号：{commands:?}"
        );
        assert_eq!(rt.pipeline_state(Pipeline::Speak), PipelineState::Idle);
    }

    #[test]
    fn pit2_stopping_twice_only_hands_shakes_once() {
        let (rt, rec) = fixture();
        bring_up(&rt, &rec, Pipeline::Speak);
        rt.stop(Pipeline::Speak);
        rec.drain();
        rt.stop(Pipeline::Speak);
        assert!(rec.drain().is_empty(), "已经停了就别再发命令");
    }

    // --- 坑 3：阀门命令带单调序号 -----------------------------------------

    #[test]
    fn pit3_gate_commands_carry_increasing_seq() {
        let (rt, rec) = fixture();
        bring_up(&rt, &rec, Pipeline::Speak);
        let mut seqs = Vec::new();
        for _ in 0..4 {
            rt.set_mic_active(!rt.mic_active());
            for cmd in rec.drain() {
                if let PipelineCommand::SetGateActive { seq, .. } = cmd {
                    seqs.push(seq);
                }
            }
        }
        assert_eq!(seqs.len(), 4, "四次切换该有四条阀门命令");
        assert!(
            seqs.windows(2).all(|w| w[1] > w[0]),
            "序号必须严格递增：{seqs:?}"
        );
    }

    #[test]
    fn pit3_gate_config_change_also_gets_a_seq() {
        let (rt, rec) = fixture();
        bring_up(&rt, &rec, Pipeline::Speak);
        rt.update_settings(|s| s.speak.gate_threshold = 0.05);
        let commands = rec.drain();
        let found = commands.iter().any(|c| {
            matches!(
                c,
                PipelineCommand::SetGateConfig { seq, config, .. }
                    if *seq > 0 && (config.threshold - 0.05).abs() < 1e-6
            )
        });
        assert!(found, "改阈值要下发带序号的阀门配置：{commands:?}");
    }

    #[test]
    fn pit3_seq_is_globally_monotonic_across_pipelines() {
        let (rt, rec) = fixture();
        bring_up(&rt, &rec, Pipeline::Speak);
        rt.set_mic_active(true);
        rt.update_settings(|s| s.speak.gate_threshold = 0.08);
        let seqs: Vec<u64> = rec
            .drain()
            .into_iter()
            .filter_map(|c| match c {
                PipelineCommand::SetGateActive { seq, .. }
                | PipelineCommand::SetGateConfig { seq, .. } => Some(seq),
                _ => None,
            })
            .collect();
        assert!(seqs.len() >= 2, "至少两条阀门命令：{seqs:?}");
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "全局单调：{seqs:?}");
    }

    // --- 坑 4：旧会话的迟到回调要丢掉 -------------------------------------

    #[test]
    fn pit4_stale_state_callback_is_dropped() {
        let (rt, rec) = fixture();
        let old = bring_up(&rt, &rec, Pipeline::Speak);
        rt.stop(Pipeline::Speak);
        // 旧会话姗姗来迟。
        rt.on_pipeline_state(Pipeline::Speak, old, PipelineState::Active);
        assert_eq!(
            rt.pipeline_state(Pipeline::Speak),
            PipelineState::Idle,
            "停了之后旧回调不能把状态拽回去"
        );
    }

    #[test]
    fn pit4_stale_subtitle_and_gate_callbacks_are_dropped() {
        let (rt, rec) = fixture();
        let old = bring_up(&rt, &rec, Pipeline::Speak);
        rt.stop(Pipeline::Speak);
        rt.on_subtitle_delta(Pipeline::Speak, old, "幽灵字幕", true, false);
        rt.on_gate_status(Pipeline::Speak, old, gate_status(true));
        let snap = rt.snapshot();
        assert!(rt.subtitle_frame().lines.is_empty(), "旧会话的字幕不该出现");
        assert!(!snap.speak.gate_open, "旧会话的阀门状态不该生效");
    }

    #[test]
    fn pit4_new_session_callbacks_still_land() {
        let (rt, rec) = fixture();
        let first = bring_up(&rt, &rec, Pipeline::Speak);
        rt.stop(Pipeline::Speak);
        let second = bring_up(&rt, &rec, Pipeline::Speak);
        assert_ne!(first, second, "重启要换新会话号");
        rt.on_subtitle_delta(Pipeline::Speak, second, "こんにちは", false, false);
        assert_eq!(rt.subtitle_frame().lines.len(), 1);
    }

    #[test]
    fn subtitle_delta_replace_swaps_the_track_line_instead_of_doubling() {
        let (rt, rec) = fixture();
        let session = bring_up(&rt, &rec, Pipeline::Speak);
        rt.on_subtitle_delta(Pipeline::Speak, session, "错误句子", false, false);
        rt.on_subtitle_delta(Pipeline::Speak, session, "订正句子", false, true);
        let text: String = rt
            .subtitle_frame()
            .lines
            .into_iter()
            .next()
            .expect("要有字幕行")
            .chars
            .iter()
            .map(|c| c.ch)
            .collect();
        assert_eq!(text, "订正句子", "replace=true 要整行替换；追加的话会叠字");
    }

    // --- 坑 5：停止时复位麦克风标志 ---------------------------------------

    #[test]
    fn pit5_stop_resets_mic_active() {
        let (rt, rec) = fixture();
        bring_up(&rt, &rec, Pipeline::Speak);
        rt.set_mic_active(true);
        assert!(rt.mic_active());
        rt.stop(Pipeline::Speak);
        assert!(!rt.mic_active(), "停流水线必须把开麦标志放下");
    }

    #[test]
    fn pit5_failure_resets_mic_active() {
        let (rt, rec) = fixture();
        let session_id = bring_up(&rt, &rec, Pipeline::Speak);
        rt.set_mic_active(true);
        rt.on_pipeline_failed(Pipeline::Speak, session_id, "连不上".into());
        assert!(!rt.mic_active(), "挂了也要复位");
        assert_eq!(rt.pipeline_state(Pipeline::Speak), PipelineState::Failed);
    }

    #[test]
    fn pit5_switching_activation_mode_resets_mic_active() {
        let (rt, rec) = fixture();
        bring_up(&rt, &rec, Pipeline::Speak);
        rt.set_mic_active(true);
        rt.update_settings(|s| s.speak.activation_mode = ActivationMode::Hold);
        assert!(!rt.mic_active(), "换激活方式不留旧的开麦状态");
    }

    // --- 坑 6：一个热键一条生效路径 ---------------------------------------

    #[test]
    fn pit6_hold_mode_press_and_release_toggle_the_gate_once_each() {
        let (rt, rec) = fixture();
        rt.update_settings(|s| s.speak.activation_mode = ActivationMode::Hold);
        bring_up(&rt, &rec, Pipeline::Speak);

        rt.on_hotkey(HotkeyEvent::SpeakPressed);
        let pressed: Vec<bool> = rec
            .drain()
            .into_iter()
            .filter_map(|c| match c {
                PipelineCommand::SetGateActive { active, .. } => Some(active),
                _ => None,
            })
            .collect();
        assert_eq!(pressed, vec![true], "按下只该开一次闸");

        // 重复的按下事件（键盘自动重复）不该再发命令。
        rt.on_hotkey(HotkeyEvent::SpeakPressed);
        assert!(rec.drain().is_empty(), "按住不放不该刷命令");

        rt.on_hotkey(HotkeyEvent::SpeakReleased);
        let released: Vec<bool> = rec
            .drain()
            .into_iter()
            .filter_map(|c| match c {
                PipelineCommand::SetGateActive { active, .. } => Some(active),
                _ => None,
            })
            .collect();
        assert_eq!(released, vec![false], "松开只该关一次闸");
    }

    #[test]
    fn pit6_toggle_mode_ignores_release() {
        let (rt, rec) = fixture();
        bring_up(&rt, &rec, Pipeline::Speak);
        rt.on_hotkey(HotkeyEvent::SpeakPressed);
        assert!(rt.mic_active());
        rt.on_hotkey(HotkeyEvent::SpeakReleased);
        assert!(rt.mic_active(), "开关模式下松手不该闭麦");
        rt.on_hotkey(HotkeyEvent::SpeakPressed);
        assert!(!rt.mic_active(), "再按一次才关");
    }

    // --- 其它 --------------------------------------------------------------

    #[test]
    fn start_without_api_key_refuses_and_explains() {
        let rt = Runtime::new(Settings::default(), Arc::new(TestClock));
        let rec = Arc::new(Recorder::default());
        rt.set_control(rec.clone());
        rt.start(Pipeline::Speak);
        assert!(rec.drain().is_empty(), "没密钥不该发启动命令");
        assert_eq!(rt.pipeline_state(Pipeline::Speak), PipelineState::Idle);
        let notices = rt.snapshot().notices;
        assert!(notices.iter().any(|n| n.text.contains("密钥")));
    }

    #[test]
    fn listen_without_target_refuses() {
        let (rt, rec) = fixture();
        rt.start(Pipeline::Listen);
        assert!(rec.drain().is_empty(), "没选程序不该启动");
        let notices = rt.snapshot().notices;
        assert!(notices.iter().any(|n| n.text.contains("程序")));
    }

    #[test]
    fn pipelines_start_with_the_configured_translation_modalities() {
        let (rt, rec) = fixture();

        rt.start(Pipeline::Speak);
        let speak = rec.drain().into_iter().find_map(|command| match command {
            PipelineCommand::Start(config) => Some(config),
            _ => None,
        });
        assert!(
            speak.expect("对外说话应启动").voice.is_some(),
            "对外说话必须始终启动 TTS"
        );

        rt.update_settings(|s| {
            s.listen.target = Some(crate::settings::ListenTarget {
                executable: "discord.exe".into(),
                display_name: "Discord".into(),
                include_process_tree: true,
            });
        });
        rec.drain();
        rt.start(Pipeline::Listen);
        let listen = rec.drain().into_iter().find_map(|command| match command {
            PipelineCommand::Start(config) => Some(config),
            _ => None,
        });
        assert!(
            listen.expect("听人说话应启动").voice.is_some(),
            "Listen 默认应启动 TTS"
        );
    }

    #[test]
    fn listen_translation_audio_switch_is_live_and_speak_audio_is_fixed_on() {
        let (rt, rec) = fixture();
        bring_up(&rt, &rec, Pipeline::Speak);
        rt.update_settings(|s| s.speak.speak_translation = false);
        assert!(rt.settings().speak.speak_translation);
        assert!(rec.drain().is_empty(), "对外说话不接受关闭 TTS");

        rt.update_settings(|s| {
            s.listen.target = Some(crate::settings::ListenTarget {
                executable: "discord.exe".into(),
                display_name: "Discord".into(),
                include_process_tree: true,
            });
        });
        rec.drain();
        let listen_session = bring_up(&rt, &rec, Pipeline::Listen);
        rt.update_settings(|s| s.listen.speak_translation = false);
        assert!(rec.drain().contains(&PipelineCommand::SetTranslationAudio {
            session_id: listen_session,
            voice: None,
            output_device: None,
        }));
    }

    #[test]
    fn hidden_translation_clears_and_stops_subtitle_events() {
        let (rt, rec) = fixture();
        let session = bring_up(&rt, &rec, Pipeline::Speak);
        rt.on_subtitle_delta(Pipeline::Speak, session, "こんにちは", false, false);
        assert_eq!(rt.subtitle_frame().lines.len(), 1);

        let events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let captured = Arc::clone(&events);
        rt.add_listener(Arc::new(move |event| captured.lock().push(event.clone())));
        rt.update_settings(|s| s.speak.show_translation = false);
        assert!(rt.subtitle_frame().lines.is_empty(), "关掉显示要立即清轨");
        assert!(events.lock().iter().any(|event| matches!(
            event,
            Event::SubtitleCleared {
                track: Track::Speak
            }
        )));

        events.lock().clear();
        rt.on_subtitle_delta(Pipeline::Speak, session, "幽灵字幕", true, false);
        assert!(rt.subtitle_frame().lines.is_empty());
        assert!(
            events
                .lock()
                .iter()
                .all(|event| !matches!(event, Event::SubtitleDelta { .. })),
            "隐藏时不应向前端广播字幕"
        );
    }

    #[test]
    fn language_change_hot_updates_instead_of_reconnecting() {
        let (rt, rec) = fixture();
        let session_id = bring_up(&rt, &rec, Pipeline::Speak);
        rt.update_settings(|s| s.set_speak_language("en"));
        let commands = rec.drain();
        assert!(
            commands.iter().any(|c| matches!(
                c,
                PipelineCommand::HotUpdate { session_id: sid, target_language: Some(l), .. }
                    if *sid == session_id && l == "en"
            )),
            "换语言走热更新：{commands:?}"
        );
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, PipelineCommand::Start(_) | PipelineCommand::Stop { .. })),
            "换语言不该重连：{commands:?}"
        );
    }

    #[test]
    fn pipelines_start_with_the_only_supported_model() {
        let (rt, rec) = fixture();
        rt.update_settings(|s| {
            s.speak.model_name = "retired-model-a".into();
            s.listen.model_name = "retired-model-b".into();
            s.listen.target = Some(crate::settings::ListenTarget {
                executable: "discord.exe".into(),
                display_name: "Discord".into(),
                include_process_tree: true,
            });
        });
        rec.drain();

        rt.start(Pipeline::Speak);
        let speak = rec.drain().into_iter().find_map(|command| match command {
            PipelineCommand::Start(config) => Some(config),
            _ => None,
        });
        assert_eq!(
            speak.expect("对外说话应启动").model_name,
            crate::catalog::DEFAULT_MODEL_NAME
        );

        rt.start(Pipeline::Listen);
        let listen = rec.drain().into_iter().find_map(|command| match command {
            PipelineCommand::Start(config) => Some(config),
            _ => None,
        });
        assert_eq!(
            listen.expect("听人说话应启动").model_name,
            crate::catalog::DEFAULT_MODEL_NAME
        );
    }

    #[test]
    fn headphone_monitor_change_is_applied_live() {
        let (rt, rec) = fixture();
        let session_id = bring_up(&rt, &rec, Pipeline::Speak);
        rt.update_settings(|s| s.speak.monitor_translation = true);
        let commands = rec.drain();

        assert!(
            commands.contains(&PipelineCommand::SetMonitorTranslation {
                session_id,
                enabled: true,
            }),
            "回听开关要即时下发：{commands:?}"
        );
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, PipelineCommand::Start(_) | PipelineCommand::Stop { .. })),
            "开回听不该重启翻译会话：{commands:?}"
        );
    }

    #[test]
    fn both_pipelines_running_advises_headphones() {
        let (rt, rec) = fixture();
        rt.update_settings(|s| {
            s.listen.target = Some(crate::settings::ListenTarget {
                executable: "discord.exe".into(),
                display_name: "Discord".into(),
                include_process_tree: true,
            });
        });
        rec.drain();
        bring_up(&rt, &rec, Pipeline::Speak);
        assert!(!rt.snapshot().headphones_advised, "只开一条不用提醒");
        bring_up(&rt, &rec, Pipeline::Listen);
        assert!(rt.snapshot().headphones_advised, "两条一起开就该提醒戴耳机");
    }

    #[test]
    fn listener_can_read_the_ledger_without_deadlocking() {
        let (rt, rec) = fixture();
        let mirror = Arc::new(Mutex::new(Vec::<PipelineState>::new()));
        {
            let rt2 = rt.clone();
            let mirror = mirror.clone();
            rt.add_listener(Arc::new(move |event| {
                if matches!(event, Event::PipelineState { .. }) {
                    // 监听器回头读快照——不许死锁。
                    mirror.lock().push(rt2.snapshot().speak.state);
                }
            }));
        }
        bring_up(&rt, &rec, Pipeline::Speak);
        assert!(!mirror.lock().is_empty(), "监听器该收到状态事件");
    }

    #[test]
    fn usage_accumulates_without_any_pricing() {
        let (rt, _rec) = fixture();
        rt.record_usage(
            "qwen3.5-livetranslate-flash-realtime",
            &TurnUsage {
                input_tokens: 120,
                output_tokens: 80,
                total_tokens: 0,
            },
        );
        rt.record_usage(
            "qwen3.5-livetranslate-flash-realtime",
            &TurnUsage {
                input_tokens: 30,
                output_tokens: 10,
                total_tokens: 0,
            },
        );
        let total = rt.usage().grand_total();
        assert_eq!(total.input_tokens, 150);
        assert_eq!(total.output_tokens, 90);
        assert_eq!(total.total_tokens, 240);
        assert_eq!(total.turns, 2);
    }

    #[test]
    fn two_subtitle_rows_render_listen_first() {
        let (rt, rec) = fixture();
        rt.update_settings(|s| {
            s.listen.target = Some(crate::settings::ListenTarget {
                executable: "vrchat.exe".into(),
                display_name: "VRChat".into(),
                include_process_tree: true,
            });
        });
        rec.drain();
        let speak = bring_up(&rt, &rec, Pipeline::Speak);
        let listen = bring_up(&rt, &rec, Pipeline::Listen);
        rt.on_subtitle_delta(Pipeline::Speak, speak, "はい", false, false);
        rt.on_subtitle_delta(Pipeline::Listen, listen, "好的", false, false);
        let frame = rt.subtitle_frame();
        assert_eq!(frame.lines.len(), 2);
        assert_eq!(frame.lines[0].track, Track::Listen, "听人说话在上面一行");
        assert_eq!(frame.lines[1].track, Track::Speak);
        assert_ne!(frame.lines[0].color, frame.lines[1].color, "两行分色");
    }

    #[test]
    fn api_key_never_reaches_the_settings_json() {
        let (rt, _rec) = fixture();
        let json = rt.settings().to_json();
        assert!(!json.contains("sk-test"), "配置文件里绝不能出现密钥");
        assert!(rt.snapshot().api_key_configured);
    }

    #[test]
    fn devices_event_only_fires_on_real_change() {
        let (rt, _rec) = fixture();
        let hits = Arc::new(AtomicU64::new(0));
        {
            let hits = hits.clone();
            rt.add_listener(Arc::new(move |event| {
                if matches!(event, Event::DevicesChanged) {
                    hits.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        let devices = DeviceSnapshot {
            inputs: vec![DeviceInfo {
                name: "麦克风".into(),
                is_default: true,
            }],
            ..Default::default()
        };
        rt.set_devices(devices.clone());
        rt.set_devices(devices);
        assert_eq!(hits.load(Ordering::SeqCst), 1, "设备没变就别刷 UI");
    }
}
