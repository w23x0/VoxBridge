//! 资源面（字幕出口）的**形状**：URI、`resources/list` 的条目、`resources/read` 的快照、
//! 以及 ticker 要的那份"谁变了"。
//!
//! 设计稿 §2.4.1。这个文件只放形状与数据——协议（方法、`_meta`、错误码、缓存提示）在
//! [`crate::mcp::resources`] / [`crate::mcp::subscriptions`]，账本在 [`crate::session`] 的后端，
//! 变更检测在 [`crate::transcript`]。
//!
//! **音频永不进协议**：资源面只有文本（`contents[0].text` 是一份快照 JSON），实时音频走
//! `vox-net` 自己的通道（§2.4.3）。这里也不存历史——字幕字有 TTL，快照天然有界。
//!
//! 一个活着的会话 = 一条资源，URI 里的 handle 由**服务端签发**（`session_open` 返回的那个）：
//! 客户端拿不到、也构造不出别人的 handle，所以资源面不需要额外的授权位（会话开起来那一步
//! 已经过了三个闸门）。

use serde_json::{json, Value};
use vox_core::event::PipelineState;

use crate::actions::EndpointId;
use crate::endpoints;

/// URI 前缀与后缀。`vox://session/<handle>/transcript`（设计稿 §2.4.1 逐字）。
pub const SCHEME: &str = "vox://session/";
pub const SUFFIX: &str = "/transcript";

/// 内容的 MIME 类型：`contents[0].text` 是一份 JSON 快照，不是纯文本。
pub const MIME: &str = "application/json";

/// 一条会话的字幕资源 URI。
pub fn uri(handle: &str) -> String {
    format!("{SCHEME}{handle}{SUFFIX}")
}

/// 从 URI 里取出 handle。**形状不对就是 `None`**——不是我们签发的 URI 不是我们的资源，
/// 但也不代表请求错了（`subscriptions/listen` 里那种 URI 只是在 ack 里被省略，见 §2.4.2）。
pub fn handle_of(uri: &str) -> Option<&str> {
    let handle = uri.strip_prefix(SCHEME)?.strip_suffix(SUFFIX)?;
    if handle.is_empty() || handle.contains('/') {
        return None;
    }
    Some(handle)
}

/// `resources/list` 里的一条（规范 `Resource`：`uri` / `name` / `title` / `description` / `mimeType`）。
///
/// `name` 是给人看的短名（两条腿都叫 `transcript`，身份在 `uri` 里）；`title` 用芯的
/// `Pipeline::label()`，不在这里另写一份"哪条腿叫什么"。
pub fn entry(endpoint: EndpointId, handle: &str) -> Value {
    let pipeline = endpoints::pipeline(endpoint);
    json!({
        "uri": uri(handle),
        "name": "transcript",
        "title": format!("{} · 字幕", pipeline.label()),
        "description": "这条会话当前可见的字幕快照（整段，不是增量）。每变一次 revision 加一。",
        "mimeType": MIME,
    })
}

/// `resources/read` 的 `contents[0]`：快照 JSON 放在 `text` 里（规范：文本内容用 `text`）。
pub fn contents(handle: &str, snapshot: &Value) -> Value {
    json!({ "uri": uri(handle), "mimeType": MIME, "text": snapshot.to_string() })
}

/// 一条字幕资源的快照（设计稿 §2.4.1 逐字的那份 JSON）。
///
/// - `text` = 芯 `SubtitleTrack::text(now_ms)` 的语义（已滤掉完全透明的字）→ 天然有界；
/// - `confirmed` / `last_delta_done` 来自 `Event::SubtitleDelta`（由控制面的监听器记下）；
/// - `revision` 单调递增，客户端靠它判断"漏没漏掉通知"（SSE 不可续传，规范要求客户端自己重发）。
pub struct Snapshot<'a> {
    pub handle: &'a str,
    pub endpoint: EndpointId,
    pub state: PipelineState,
    pub revision: u64,
    pub notify_ms: u32,
    pub text: &'a str,
    pub confirmed: Option<&'a str>,
    pub last_delta_done: bool,
    pub updated_at_ms: u64,
}

impl Snapshot<'_> {
    pub fn to_json(&self) -> Value {
        json!({
            "session": self.handle,
            "endpoint": self.endpoint.as_str(),
            "track": endpoints::pipeline(self.endpoint).track(),
            "state": self.state,
            "revision": self.revision,
            "notify_ms": self.notify_ms,
            "text": self.text,
            "confirmed": self.confirmed,
            "last_delta_done": self.last_delta_done,
            "updated_at_ms": self.updated_at_ms,
        })
    }
}

/// 一条活着的字幕资源**现在的 revision**。
///
/// ticker 只拿它比大小，不读内容：通知里只有 URI（规范），内容由客户端自己 `resources/read`。
pub struct ResourceState {
    pub uri: String,
    pub revision: u64,
}

/// 一次资源轮询的结果。
///
/// `notify_ms` 是下一次该等多久（＝ `Settings.control.transcript_notify_ms`，**现读**，所以
/// 用户在设置里改了间隔下一个 tick 就生效）；`list_revision` 变了 = 活着的资源集合变了
/// （会话开/关）；`resources` 只有活着的那些。
pub struct ResourceTick {
    pub notify_ms: u32,
    pub list_revision: u64,
    pub resources: Vec<ResourceState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_round_trips_and_rejects_foreign_shapes() {
        let uri = uri("s_1f4c8a2b");
        assert_eq!(uri, "vox://session/s_1f4c8a2b/transcript");
        assert_eq!(handle_of(&uri), Some("s_1f4c8a2b"));

        for foreign in [
            "vox://session//transcript",
            "vox://session/s_a/b/transcript",
            "vox://session/s_a",
            "vox://session/s_a/transcript/",
            "file:///project/config.json",
            "VOX://session/s_a/transcript",
        ] {
            assert_eq!(handle_of(foreign), None, "{foreign} 不是我们签发的形状");
        }
    }

    #[test]
    fn snapshot_carries_every_documented_cell() {
        let snapshot = Snapshot {
            handle: "s_a",
            endpoint: EndpointId::Speak,
            state: PipelineState::Active,
            revision: 17,
            notify_ms: 250,
            text: "Hello",
            confirmed: Some("Hello"),
            last_delta_done: false,
            updated_at_ms: 18_342,
        }
        .to_json();

        assert_eq!(snapshot["session"], json!("s_a"));
        assert_eq!(snapshot["endpoint"], json!("speak"));
        assert_eq!(snapshot["track"], json!("speak"));
        assert_eq!(snapshot["state"], json!("active"));
        assert_eq!(snapshot["revision"], json!(17));
        assert_eq!(snapshot["notify_ms"], json!(250));
        assert_eq!(snapshot["text"], json!("Hello"));
        assert_eq!(snapshot["confirmed"], json!("Hello"));
        assert_eq!(snapshot["last_delta_done"], json!(false));
        assert_eq!(snapshot["updated_at_ms"], json!(18_342));

        // `confirmed` 没有可用前缀时是 `null`（不是缺格、也不是空串）。
        let bare = Snapshot {
            handle: "s_b",
            endpoint: EndpointId::Listen,
            state: PipelineState::Idle,
            revision: 0,
            notify_ms: 250,
            text: "",
            confirmed: None,
            last_delta_done: true,
            updated_at_ms: 0,
        }
        .to_json();
        assert_eq!(bare["confirmed"], Value::Null);
        assert_eq!(bare["track"], json!("listen"));
        assert_eq!(bare["text"], json!(""));
    }
}
