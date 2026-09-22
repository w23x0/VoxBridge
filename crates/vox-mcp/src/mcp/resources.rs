//! 资源面的两条方法：`resources/list` 与 `resources/read`。
//!
//! 形状（URI、快照有哪些格）在 [`crate::resources`]，数据在 [`ControlBackend`]——这一层只做
//! 协议该做的事：校验参数、翻错误码、补 `ttlMs` / `cacheScope` / `resultType` / `_meta`。
//!
//! 两条规范硬点：
//!
//! - **缓存**：两条结果都**必须**带缓存提示（`server/utilities/caching`），定值是
//!   `ttlMs: 0` + `cacheScope: "private"`（设计稿 §2.3.2）——字幕是用户私有数据，而且
//!   **不许缓存住旧字幕**：客户端每次要都该重取。
//! - **资源不存在**：错误码是 **`-32602`**（规范 Major 6 换掉了旧的 `-32002`），
//!   `data.uri` 指出是哪一条；**不许**回一个空的 `contents` 数组（规范：空数组有歧义）。

use serde_json::{json, Map, Value};

use crate::handlers::ControlBackend;
use crate::jsonrpc::{self, code, ErrorObject, Request};
use crate::mcp::{cache, respond};

/// `resources/list`：现在活着的会话各自一条字幕资源。
///
/// 不分页（本服务端最多两条资源，没有 `nextCursor`）；不因授权位增删（缺授权是**调用时**失败，
/// 不做"藏资源"——与 `tools/list` 同一条口径）。
pub fn list_call(request: &Request, backend: Option<&mut dyn ControlBackend>) -> Value {
    let Some(backend) = backend else {
        return no_backend(&request.id);
    };
    let mut result = Map::new();
    result.insert("resources".to_string(), backend.list_resources());
    cache::RESOURCES_LIST.apply(&mut result);
    respond(&request.id, result)
}

/// `resources/read`：一条字幕资源的快照。
pub fn read_call(request: &Request, backend: Option<&mut dyn ControlBackend>) -> Value {
    let Some(uri) = params(request)
        .and_then(|params| params.get("uri"))
        .and_then(Value::as_str)
    else {
        return jsonrpc::error_result(
            Some(&request.id),
            &ErrorObject::with_data(
                code::INVALID_PARAMS,
                "resources/read 必须带 uri（字符串）",
                json!({ "path": "uri" }),
            ),
        );
    };

    let Some(backend) = backend else {
        return no_backend(&request.id);
    };

    match backend.read_resource(uri) {
        Some(contents) => {
            let mut result = Map::new();
            result.insert("contents".to_string(), json!([contents]));
            cache::RESOURCES_READ.apply(&mut result);
            respond(&request.id, result)
        }
        // 规范 MUST：资源不存在 → `-32602`，且**不许**回空 `contents`。
        None => jsonrpc::error_result(
            Some(&request.id),
            &ErrorObject::with_data(
                code::INVALID_PARAMS,
                "资源不存在：这个 URI 不是本服务端签发的，或那条会话已经关了",
                json!({ "uri": uri }),
            ),
        ),
    }
}

/// 后端没接上：如实报 `-32603`，不假装"一条资源都没有"（那会让客户端以为设备上真的没有会话）。
fn no_backend(id: &Value) -> Value {
    jsonrpc::error_result(
        Some(id),
        &ErrorObject::new(
            code::INTERNAL_ERROR,
            "控制面后端未接入：vox-mcp 只提供协议面，账本与设备由外壳注入",
        ),
    )
}

fn params(request: &Request) -> Option<&Map<String, Value>> {
    request.params.as_ref().and_then(Value::as_object)
}
