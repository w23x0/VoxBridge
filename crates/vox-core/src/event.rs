//! 内核 → 外壳的事件。
//!
//! 账本每次变动都广播一条事件，设置界面和悬浮窗都是订阅者。谁都不许自己存一份
//! 状态副本：要么订事件，要么找 [`crate::runtime::Runtime`] 拿快照。

use serde::Serialize;

use crate::gate::GateStatus;
use crate::latency::LatencySnapshot;
use crate::settings::Settings;
use crate::subtitle::Track;
use crate::usage::UsageLedger;

/// 哪条流水线。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pipeline {
    /// 对外说话。
    Speak,
    /// 听人说话。
    Listen,
}

impl Pipeline {
    pub fn label(self) -> &'static str {
        match self {
            Self::Speak => "对外说话",
            Self::Listen => "听人说话",
        }
    }

    /// 这条流水线的字幕走哪条轨道。
    pub fn track(self) -> Track {
        match self {
            Self::Listen => Track::Listen,
            Self::Speak => Track::Speak,
        }
    }
}

/// 一条流水线的运行阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineState {
    /// 没开。
    Idle,
    /// 正在起（开设备、连模型）。
    Starting,
    /// 连上了，等着说话。
    Ready,
    /// 正在传音频。
    Active,
    /// 断了，正在重连。
    Reconnecting,
    /// 挂了。
    Failed,
}

impl PipelineState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "待机",
            Self::Starting => "启动中",
            Self::Ready => "已就绪",
            Self::Active => "运行中",
            Self::Reconnecting => "重连中",
            Self::Failed => "错误",
        }
    }

    pub fn is_running(self) -> bool {
        !matches!(self, Self::Idle | Self::Failed)
    }
}

/// 提示信息的轻重。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// 一条要给用户看的提示。
#[derive(Debug, Clone, Serialize)]
pub struct Notice {
    pub severity: Severity,
    pub text: String,
    /// 哪条流水线引起的；`None` 表示全局。
    pub pipeline: Option<Pipeline>,
}

impl Notice {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            text: text.into(),
            pipeline: None,
        }
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            text: text.into(),
            pipeline: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            text: text.into(),
            pipeline: None,
        }
    }

    pub fn on(mut self, pipeline: Pipeline) -> Self {
        self.pipeline = Some(pipeline);
        self
    }
}

/// 账本变动事件。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// 设置整体变了。
    SettingsChanged { settings: Box<Settings> },
    /// 某条流水线的阶段变了。
    PipelineState {
        pipeline: Pipeline,
        state: PipelineState,
    },
    /// 阀门状态（驱动 UI 的实时电平条）。发得很频，订阅方要能扛住。
    GateStatus {
        pipeline: Pipeline,
        status: GateStatus,
    },
    /// 某条流水线的延迟与队列健康度更新。
    LatencyChanged {
        pipeline: Pipeline,
        latency: Box<LatencySnapshot>,
    },
    /// 来了一段译文文字。
    SubtitleDelta {
        track: Track,
        text: String,
        /// 这一段是否说完了。
        done: bool,
        /// 服务端把整句重写了（改译、纠错）。`true` 时 `text` 是完整的当前句，
        /// 字幕层要**整行替换**而不是追加——否则订正会让屏幕上叠出一坨重复字。
        replace: bool,
    },
    /// 服务端回报的**源文识别语种**（只给用户看，不改任何行为）。
    /// 自动识别下才有意义：识别成什么语言，让用户看见，说错了能察觉。
    SourceDetected { track: Track, language: String },
    /// 字幕被清空。
    SubtitleCleared { track: Track },
    /// token 用量涨了。
    UsageChanged { usage: Box<UsageLedger> },
    /// 麦克风开关状态（跟着热键走）。
    MicActive { active: bool },
    /// 设备列表刷新了。
    DevicesChanged,
    /// 给用户看的提示。
    Notice { notice: Notice },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipelines_use_their_own_subtitle_rows() {
        assert_eq!(Pipeline::Speak.track(), Track::Speak);
        assert_eq!(Pipeline::Listen.track(), Track::Listen);
    }

    #[test]
    fn only_idle_and_failed_count_as_not_running() {
        assert!(!PipelineState::Idle.is_running());
        assert!(!PipelineState::Failed.is_running());
        assert!(PipelineState::Reconnecting.is_running());
        assert!(PipelineState::Active.is_running());
    }

    #[test]
    fn events_serialize_with_a_kind_tag() {
        let json = serde_json::to_string(&Event::PipelineState {
            pipeline: Pipeline::Speak,
            state: PipelineState::Ready,
        })
        .unwrap();
        assert!(json.contains(r#""kind":"pipeline_state""#), "{json}");
        assert!(json.contains(r#""pipeline":"speak""#), "{json}");
        assert!(json.contains(r#""state":"ready""#), "{json}");
    }

    #[test]
    fn notice_can_be_attributed_to_a_pipeline() {
        let n = Notice::error("连不上").on(Pipeline::Listen);
        assert_eq!(n.pipeline, Some(Pipeline::Listen));
        assert_eq!(n.severity, Severity::Error);
        assert!(Notice::info("x").pipeline.is_none());
    }
}
