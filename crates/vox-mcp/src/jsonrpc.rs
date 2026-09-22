//! JSON-RPC 2.0 帧 + MCP 规范错误码常量。
//!
//! 这一层只认帧：不认方法、不认 `_meta`、不认账本。
//!
//! 规范出处（2026-07-28）：`basic/index` 的 Messages / Error Codes / `resultType`；
//! `basic/versioning` 的 `UnsupportedProtocolVersionError`。

use serde_json::{Map, Value};

/// 版本字面量。请求与响应都必须逐字带 `"jsonrpc": "2.0"`。
pub const VERSION: &str = "2.0";

/// MCP 与 JSON-RPC 的错误码表。**一个数字都不自造**。
///
/// 规范把 `-32000..=-32099` 留给实现、把 `-32020..=-32099` 收归规范
/// （`basic/index#error-codes`）：`-32000..=-32019` 是历史遗留、新实现不该用；
/// 区间之外的新码 SHOULD 分配在 `-32768..=-32000` 之外。我们的领域失败（设备不支持、
/// 没授权、token 过期…）**不是** JSON-RPC 错误，走工具结果的 `isError`
/// （`structuredContent.error.code` 是 snake_case 字符串，见 [`crate::actions::DomainErrorCode`]）：
/// 模型要能看见失败并改参数，JSON-RPC 错误是给宿主/传输层看的。
pub mod code {
    /// 请求不是合法 JSON。
    pub const PARSE_ERROR: i32 = -32700;
    /// 不是合法 JSON-RPC 请求：缺 `jsonrpc`、`method` 不是字符串、`id` 是 `null`、`params` 不是对象。
    pub const INVALID_REQUEST: i32 = -32600;
    /// 方法不存在（HTTP 传输上同时是 404）。
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// 参数不合法：缺必填、枚举越界、未知键、数值越界，以及 `_meta` 缺必填字段。
    pub const INVALID_PARAMS: i32 = -32602;
    /// 服务端内部错误。
    pub const INTERNAL_ERROR: i32 = -32603;
    /// `Mcp-*` 请求头与 body 不一致（HTTP 传输层用，`streamable-http#server-validation`）。
    pub const HEADER_MISMATCH: i32 = -32020;
    /// 该请求需要客户端声明它没有的能力，`data.requiredCapabilities` 列出缺的那些。
    pub const MISSING_REQUIRED_CLIENT_CAPABILITY: i32 = -32021;
    /// 请求里的 `protocolVersion` 不是服务端支持的版本，`data.supported` / `data.requested` 给出对照。
    pub const UNSUPPORTED_PROTOCOL_VERSION: i32 = -32022;
}

/// 一条 JSON-RPC 错误。
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    /// `-32602` 用它说明"哪一格不对"，`-32022` 用它带 `{supported, requested}`。
    pub data: Option<Value>,
}

impl ErrorObject {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(code: i32, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }
}

/// 一条请求帧。
#[derive(Debug, Clone)]
pub struct Request {
    /// 规范：请求 MUST 带 `string | number` 的 id，且 **MUST NOT 是 `null`**。
    pub id: Value,
    pub method: String,
    /// 原样保留（`_meta` 就住在这里，由 [`crate::mcp::meta`] 校验）。
    pub params: Option<Value>,
}

/// 收到的一条消息。通知不回响应（规范：接收方 MUST NOT 回通知）。
#[derive(Debug, Clone)]
pub enum Incoming {
    Request(Request),
    Notification {
        method: String,
        params: Option<Value>,
    },
}

/// 解析一条 JSON-RPC 消息。
///
/// 判据只有规范那几条：`jsonrpc` 必须是 `"2.0"`、`method` 必须是字符串、`params` 若存在必须是
/// 对象、`id` 缺省即通知、`id` 不许是 `null`。**不在这里校验 `_meta`**——那是 [`crate::mcp`] 的事。
pub fn parse(message: &Value) -> Result<Incoming, ErrorObject> {
    let Some(object) = message.as_object() else {
        return Err(ErrorObject::new(
            code::INVALID_REQUEST,
            "请求必须是 JSON 对象",
        ));
    };

    match object.get("jsonrpc").and_then(Value::as_str) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(ErrorObject::with_data(
                code::INVALID_REQUEST,
                format!("jsonrpc 必须是 \"{VERSION}\""),
                serde_json::json!({ "jsonrpc": other }),
            ));
        }
        None => {
            return Err(ErrorObject::new(
                code::INVALID_REQUEST,
                format!("缺少 jsonrpc 字段（必须是字符串 \"{VERSION}\"）"),
            ));
        }
    }

    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Err(ErrorObject::new(
            code::INVALID_REQUEST,
            "method 必须是字符串",
        ));
    };

    let params = object.get("params").cloned();
    if params.as_ref().is_some_and(|p| !p.is_object()) {
        return Err(ErrorObject::new(code::INVALID_REQUEST, "params 必须是对象"));
    }

    match object.get("id") {
        // 没有 id = 通知；方法名与参数照收，回什么都不回。
        None => Ok(Incoming::Notification {
            method: method.to_string(),
            params,
        }),
        Some(Value::Null) => Err(ErrorObject::new(code::INVALID_REQUEST, "id 不许是 null")),
        Some(id) => Ok(Incoming::Request(Request {
            id: id.clone(),
            method: method.to_string(),
            params,
        })),
    }
}

/// 结果响应。**必带 `resultType`**（规范 Major 8：所有结果都要有它），值只有 `"complete"`——
/// v1 不实现 MRTR（不用 `"input_required"`），也不产生 `"task"`（不广告 Tasks 扩展）。
///
/// `_meta.serverInfo` 由 [`crate::mcp`] 补（它才知道服务端身份），本函数只管帧与 `resultType`。
pub fn complete_result(id: &Value, mut fields: Map<String, Value>) -> Value {
    fields.insert(
        "resultType".to_string(),
        Value::String("complete".to_string()),
    );
    serde_json::json!({ "jsonrpc": VERSION, "id": id, "result": Value::Object(fields) })
}

/// 错误响应。`id` 缺省只在"连 id 都读不出来"时发生（规范允许）。
pub fn error_result(id: Option<&Value>, error: &ErrorObject) -> Value {
    let mut body = Map::new();
    body.insert("code".to_string(), Value::from(error.code));
    body.insert("message".to_string(), Value::String(error.message.clone()));
    if let Some(data) = &error.data {
        body.insert("data".to_string(), data.clone());
    }
    let mut response = Map::new();
    response.insert("jsonrpc".to_string(), Value::String(VERSION.to_string()));
    if let Some(id) = id {
        response.insert("id".to_string(), id.clone());
    }
    response.insert("error".to_string(), Value::Object(body));
    Value::Object(response)
}
