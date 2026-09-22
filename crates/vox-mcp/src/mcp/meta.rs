//! 每请求 `_meta` 的解析与校验。
//!
//! 2026-07-28 把协议做成**无状态**：没有握手、没有 session，版本与客户端能力改成**每个请求**
//! 从 `_meta` 里带（`basic/index` 的 Per-request protocol fields）。服务端处理每个请求都从零开始，
//! 不许从连接推断任何上下文。

use serde_json::{json, Value};

use crate::jsonrpc::{code, ErrorObject};

/// 本服务端讲的版本。只有这一个。
///
/// **不做老式 `initialize` 双代兼容**：2026-07-28 是破坏性版本，双代兼容是双倍协议面，
/// 而"宿主覆盖低"是我们已知并接受的代价（`docs/architecture/DIRECTIONS.md` §9.1-3）。
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// 支持的版本表（登记在 `server/discover` 的 `supportedVersions` 与 `-32022` 的 `data.supported`）。
pub const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION];

/// `_meta` 里 MCP 保留的键（`basic/index#_meta`：`io.modelcontextprotocol/` 前缀为规范保留）。
pub mod key {
    pub const PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
    pub const CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
    pub const CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
    pub const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
    /// 订阅流上的每条消息都带它，值 = `subscriptions/listen` 请求的 JSON-RPC id
    /// （`basic/patterns/subscriptions`：无状态协议只有这一个关联办法）。
    pub const SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";
}

/// 一条请求的 `_meta`。
///
/// 刻意**不**解析 `io.modelcontextprotocol/logLevel`：`logging` 已弃用（规范 Deprecated 1），
/// 我们的日志走 stderr / `tracing`，也不会给未声明它的请求发 `notifications/message`。
#[derive(Debug, Clone)]
pub struct RequestMeta {
    pub protocol_version: String,
    /// 客户端身份。只用于显示与日志——规范说得很直白：它自报的，别拿它改变行为或做安全判断。
    pub client_info: Option<Value>,
    /// 客户端能力。服务端**不许**依赖客户端没声明过的能力（需要时回 `-32021`）。
    pub client_capabilities: Value,
}

/// 校验并取出 `_meta`。
///
/// - 缺 `protocolVersion` / `clientCapabilities`（或类型不对）→ `-32602`（规范：HTTP 上还必须是 400）；
/// - 版本不在 [`SUPPORTED_VERSIONS`] 里 → `-32022`，`data.supported` / `data.requested` 给出对照。
pub fn validate(params: Option<&Value>) -> Result<RequestMeta, ErrorObject> {
    let params = match params {
        None => return Err(invalid("缺少 params：每请求的 _meta 住在 params 里")),
        Some(value) => value
            .as_object()
            .ok_or_else(|| invalid("params 必须是对象"))?,
    };

    let meta = match params.get("_meta") {
        None => {
            return Err(invalid(format!(
                "缺少 params._meta：每请求都要带 {} 与 {}",
                key::PROTOCOL_VERSION,
                key::CLIENT_CAPABILITIES
            )));
        }
        Some(value) => value
            .as_object()
            .ok_or_else(|| invalid("params._meta 必须是对象"))?,
    };

    let protocol_version = match meta.get(key::PROTOCOL_VERSION) {
        Some(Value::String(version)) => version.clone(),
        Some(_) => return Err(invalid(format!("{} 必须是字符串", key::PROTOCOL_VERSION))),
        None => return Err(invalid(format!("_meta 缺 {}", key::PROTOCOL_VERSION))),
    };

    if !SUPPORTED_VERSIONS.contains(&protocol_version.as_str()) {
        return Err(ErrorObject::with_data(
            code::UNSUPPORTED_PROTOCOL_VERSION,
            "不支持的协议版本",
            json!({ "supported": SUPPORTED_VERSIONS, "requested": protocol_version }),
        ));
    }

    let client_capabilities = match meta.get(key::CLIENT_CAPABILITIES) {
        Some(value) if value.is_object() => value.clone(),
        Some(_) => return Err(invalid(format!("{} 必须是对象", key::CLIENT_CAPABILITIES))),
        None => return Err(invalid(format!("_meta 缺 {}", key::CLIENT_CAPABILITIES))),
    };

    Ok(RequestMeta {
        protocol_version,
        client_info: meta.get(key::CLIENT_INFO).cloned(),
        client_capabilities,
    })
}

/// `-32602`：把缺的那一格写进 `message` 与 `data.key`，宿主/模型才知道该补什么。
fn invalid(message: impl Into<String>) -> ErrorObject {
    let message = message.into();
    ErrorObject::with_data(
        code::INVALID_PARAMS,
        message.clone(),
        json!({ "why": message }),
    )
}
