//! `subscriptions/listen`：本版协议**唯一**的推送通道（规范 Major 4）。
//!
//! 它取代了老版的 HTTP GET 端点与 `resources/subscribe`。规矩（`basic/patterns/subscriptions`）：
//!
//! 1. 客户端在请求里给一份**过滤**（`params.notifications`），服务端**不许**发客户端没勾的类型；
//! 2. 第一条消息**必须**是 `notifications/subscriptions/acknowledged`，它的 `notifications` 是
//!    服务端**同意**的那部分（不支持的省略，不是拒绝整条请求）；
//! 3. 之后每条通知都带 `_meta["io.modelcontextprotocol/subscriptionId"]`，值 = `listen` 请求的
//!    JSON-RPC id（无状态协议没有别的关联办法）；
//! 4. 服务端主动收流时 **SHOULD** 先回一条 result（`resultType: "complete"` + 同一个
//!    `subscriptionId`），客户端据此区分"干净结束"与"意外断开"。
//!
//! 这个文件是**服务端半边**：它把一条 listen 请求变成一份 [`Subscription`]（认得哪条 URI、
//! 已经发到哪个 revision），并且**自己造通知**。传输面只负责把它按 SSE 写出去——ticker 拿到的
//! 只是 [`crate::resources::ResourceTick`]，看不懂任何资源语义。

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::jsonrpc::{self, code, ErrorObject, Request};
use crate::mcp::meta;
use crate::resources::{self, ResourceTick};

/// 服务端**同意**的过滤（ack 里回的就是这一份，逐字）。
///
/// 只包含我们真的会发的两类：`resourcesListChanged`（会话开/关）与 `resourceSubscriptions`
/// （字幕变更）。`toolsListChanged` / `promptsListChanged` **不勾**——5 个工具编译期恒定、
/// 也没有 prompts，勾了就是撒谎（"位必须是事实"）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub resources_list_changed: bool,
    /// 我们形状的 URI（`vox://session/<handle>/transcript`），原样回在 ack 里。
    pub resource_subscriptions: Vec<String>,
}

impl Filter {
    /// 线上形状（ack 的 `params.notifications`）。空过滤就是 `{}`。
    pub fn to_json(&self) -> Value {
        let mut filter = Map::new();
        if self.resources_list_changed {
            filter.insert("resourcesListChanged".to_string(), Value::Bool(true));
        }
        if !self.resource_subscriptions.is_empty() {
            filter.insert(
                "resourceSubscriptions".to_string(),
                json!(self.resource_subscriptions),
            );
        }
        Value::Object(filter)
    }
}

/// 一条订阅流的服务端半边。
///
/// `watermarks` / `list_watermark` 是**这条流自己**发到哪的水位：资源变了就涨 `revision`，
/// 水位落后就补一条通知。用"水位"而不是"revision 变没变"是因为 `resources/read` 也会观察、
/// 也会涨 `revision`——比"变没变"会让订阅者漏掉一次通知（见 `session.rs::read_resource`）。
///
/// 水位在**订阅那一刻**（[`accept`] 的 `baseline`）就对齐，见那里的注释。
#[derive(Debug)]
pub struct Subscription {
    /// `listen` 请求的 JSON-RPC id：每条通知的 `subscriptionId` 就是它。
    id: Value,
    granted: Filter,
    /// URI → 已经发出去的 revision（初始值 = 订阅那一刻的）。
    watermarks: BTreeMap<String, u64>,
    /// 已经发出去的 `list_revision`（初始值 = 订阅那一刻的）。
    list_watermark: u64,
}

impl Subscription {
    /// 这条订阅的 id（= listen 请求的 id）。
    pub fn id(&self) -> &Value {
        &self.id
    }

    /// 服务端同意的过滤（测试与传输面都可能想看）。
    pub fn granted(&self) -> &Filter {
        &self.granted
    }

    /// 第一条消息：`notifications/subscriptions/acknowledged`（规范 MUST，且 MUST 是第一条）。
    pub fn acknowledged(&self) -> Value {
        json!({
            "jsonrpc": jsonrpc::VERSION,
            "method": "notifications/subscriptions/acknowledged",
            "params": {
                "_meta": { meta::key::SUBSCRIPTION_ID: self.id },
                "notifications": self.granted.to_json(),
            },
        })
    }

    /// 服务端主动收流前的那条 result（规范 SHOULD，`basic/patterns/subscriptions#graceful-closure`）。
    pub fn closed(&self) -> Value {
        let mut result = Map::new();
        result.insert(
            "_meta".to_string(),
            json!({ meta::key::SUBSCRIPTION_ID: self.id }),
        );
        jsonrpc::complete_result(&self.id, result)
    }

    /// 一拍：按水位算这条流该发哪些通知（顺序 = 先列表后资源，确定顺序）。
    ///
    /// 水位在**订阅那一刻**就对齐了（[`accept`] 的 `baseline`），这里只管发。唯一还会
    /// "只对齐不发"的是**基线之后才出现**的资源（客户端先订阅、后开会话，设计稿 §2.4.2）：
    /// 它第一次露面时对齐水位，不补一条 `updated`——那条会话的字客户端一个字都没见过，
    /// `list_changed` 已经告诉它"集合变了"，它自己去 `resources/read`。
    pub fn updates(&mut self, tick: &ResourceTick) -> Vec<Value> {
        let mut messages = Vec::new();

        if tick.list_revision > self.list_watermark {
            self.list_watermark = tick.list_revision;
            if self.granted.resources_list_changed {
                messages.push(list_changed(&self.id));
            }
        }

        for resource in &tick.resources {
            if !self
                .granted
                .resource_subscriptions
                .iter()
                .any(|uri| uri == &resource.uri)
            {
                continue;
            }
            match self.watermarks.get(&resource.uri) {
                None => {
                    self.watermarks
                        .insert(resource.uri.clone(), resource.revision);
                }
                Some(seen) if resource.revision > *seen => {
                    self.watermarks
                        .insert(resource.uri.clone(), resource.revision);
                    messages.push(updated(&self.id, &resource.uri));
                }
                Some(_) => {}
            }
        }

        messages
    }
}

/// 解析一条 `subscriptions/listen` 请求 → 服务端同意的子集，**并把水位对齐到 `baseline`**。
///
/// `baseline` 必须是**回 ack 之前这一刻**的账本（`ControlBackend::poll_resources()`）。不能拖到
/// ticker 的第一拍：客户端拿到 ack 之后立刻做的事（说话、关会话）如果落在"订阅"与"第一拍"
/// 之间，就会被那一拍当成基线吞掉——**那次变化永远不通知**（不是慢一拍，是丢一条：水位与
/// 变更检测的指纹都在那一拍才登记，之后状态稳定下来就再没有第二次机会）。
/// 高负载下这条窗口真会踩中：`tests/lifecycle.rs` 的 SSE 用例偶发红就是它（实测到 30 s 上限
/// 也没有那条通知）；`tests/resources.rs` 有一条用例把这条契约钉死。
///
/// 三种失败都是 `-32602`（参数不满足 schema）：没有 `params.notifications`、它不是一个对象、
/// 或者已知键的类型不对（`resourcesListChanged` 不是布尔、`resourceSubscriptions` 不是字符串数组）。
/// **不认识的键忽略**：规范没有把 `SubscriptionFilter` 关成 `additionalProperties: false`，
/// 老客户端多带一格不该让整条请求失败。
///
/// 形状不是 `vox://session/<handle>/transcript` 的 URI **不回绝请求**，只是在同意的那份里
/// **省略**（规范：ack 反映服务端同意的那部分）。订阅一个还没开的会话的 URI 则是**接受**——
/// 客户端可以先订阅再开（设计稿 §2.4.2）。
pub fn accept(request: &Request, baseline: &ResourceTick) -> Result<Subscription, ErrorObject> {
    let params = request.params.as_ref().and_then(Value::as_object);
    let Some(notifications) = params
        .and_then(|params| params.get("notifications"))
        .and_then(Value::as_object)
    else {
        return Err(invalid(
            "subscriptions/listen 必须带 params.notifications（SubscriptionFilter）",
            json!({ "path": "notifications" }),
        ));
    };

    let resources_list_changed = match notifications.get("resourcesListChanged") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(other) => {
            return Err(invalid(
                "resourcesListChanged 必须是布尔",
                json!({ "path": "notifications.resourcesListChanged", "got": other }),
            ))
        }
    };

    let resource_subscriptions = match notifications.get("resourceSubscriptions") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut granted = Vec::new();
            for item in items {
                let Some(uri) = item.as_str() else {
                    return Err(invalid(
                        "resourceSubscriptions 必须是字符串数组",
                        json!({ "path": "notifications.resourceSubscriptions", "got": item }),
                    ));
                };
                if resources::handle_of(uri).is_some() {
                    granted.push(uri.to_string());
                }
            }
            granted
        }
        Some(other) => {
            return Err(invalid(
                "resourceSubscriptions 必须是字符串数组",
                json!({ "path": "notifications.resourceSubscriptions", "got": other }),
            ))
        }
    };

    Ok(Subscription {
        id: request.id.clone(),
        granted: Filter {
            resources_list_changed,
            resource_subscriptions,
        },
        // 水位 = 基线：**这一刻正在的字不算"变了"**（刚订阅上来的客户端自己会 `resources/read`），
        // 这一刻之后变的才算——包括"ack 还在路上时说的话"。
        watermarks: baseline
            .resources
            .iter()
            .map(|resource| (resource.uri.clone(), resource.revision))
            .collect(),
        list_watermark: baseline.list_revision,
    })
}

/// `notifications/resources/updated`：一条被订阅的资源变了，客户端该重读它。
fn updated(id: &Value, uri: &str) -> Value {
    json!({
        "jsonrpc": jsonrpc::VERSION,
        "method": "notifications/resources/updated",
        "params": { "_meta": { meta::key::SUBSCRIPTION_ID: id }, "uri": uri },
    })
}

/// `notifications/resources/list_changed`：能读的资源集合变了（会话开/关）。
fn list_changed(id: &Value) -> Value {
    json!({
        "jsonrpc": jsonrpc::VERSION,
        "method": "notifications/resources/list_changed",
        "params": { "_meta": { meta::key::SUBSCRIPTION_ID: id } },
    })
}

fn invalid(message: &str, data: Value) -> ErrorObject {
    ErrorObject::with_data(code::INVALID_PARAMS, message, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceState;

    fn request(notifications: Value) -> Request {
        Request {
            id: json!("listen-1"),
            method: "subscriptions/listen".to_string(),
            params: Some(json!({ "notifications": notifications })),
        }
    }

    fn tick(list_revision: u64, resources: &[(&str, u64)]) -> ResourceTick {
        ResourceTick {
            notify_ms: 250,
            list_revision,
            resources: resources
                .iter()
                .map(|(uri, revision)| ResourceState {
                    uri: (*uri).to_string(),
                    revision: *revision,
                })
                .collect(),
        }
    }

    #[test]
    fn the_acknowledgment_reports_only_what_we_will_send() {
        let subscription = accept(
            &request(json!({
                "toolsListChanged": true,
                "promptsListChanged": true,
                "resourcesListChanged": true,
                "resourceSubscriptions": [
                    "vox://session/s_a/transcript",
                    "file:///project/config.json",
                    "vox://session//transcript",
                ],
            })),
            &tick(0, &[]),
        )
        .expect("接受");

        let acknowledged = subscription.acknowledged();
        assert_eq!(
            acknowledged["method"],
            json!("notifications/subscriptions/acknowledged")
        );
        assert_eq!(
            acknowledged["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
            json!("listen-1")
        );
        // 同意的部分：我们真会发的两类；别人的 URI 与工具/提示列表的变化都不在里面。
        assert_eq!(
            acknowledged["params"]["notifications"],
            json!({
                "resourcesListChanged": true,
                "resourceSubscriptions": ["vox://session/s_a/transcript"],
            })
        );

        // 一个都不勾：同意的那份是空对象（合法，不是拒绝）。
        let nothing = accept(&request(json!({})), &tick(0, &[])).expect("接受");
        assert_eq!(nothing.acknowledged()["params"]["notifications"], json!({}));
    }

    #[test]
    fn a_bad_filter_is_invalid_params() {
        for (what, notifications) in [
            (
                "resourcesListChanged 不是布尔",
                json!({ "resourcesListChanged": "yes" }),
            ),
            (
                "resourceSubscriptions 不是数组",
                json!({ "resourceSubscriptions": "vox://session/s_a/transcript" }),
            ),
            (
                "resourceSubscriptions 里混了非字符串",
                json!({ "resourceSubscriptions": [1] }),
            ),
        ] {
            let error = accept(&request(notifications), &tick(0, &[])).expect_err(what);
            assert_eq!(error.code, code::INVALID_PARAMS, "{what}");
        }

        // 连 `notifications` 都没有（schema 里它是必填）。
        let bare = Request {
            id: json!(1),
            method: "subscriptions/listen".to_string(),
            params: Some(json!({})),
        };
        assert_eq!(
            accept(&bare, &tick(0, &[]))
                .expect_err("缺 notifications")
                .code,
            -32602
        );
    }

    #[test]
    fn watermarks_decide_what_is_sent() {
        let uri = "vox://session/s_a/transcript";
        // 基线 = 订阅那一刻：会话正开着（`list_revision` 1），字幕还没动（`revision` 0）。
        let mut subscription = accept(
            &request(json!({
                "resourcesListChanged": true,
                "resourceSubscriptions": [uri],
            })),
            &tick(1, &[(uri, 0)]),
        )
        .expect("接受");

        // 一条都没变：不发，而且**不为"它本来就有字"补发**（基线里那 0 就是它）。
        assert!(subscription.updates(&tick(1, &[(uri, 0)])).is_empty());

        // 字幕变了：一条 updated，带同一个 subscriptionId。
        let messages = subscription.updates(&tick(1, &[(uri, 3)]));
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0]["method"],
            json!("notifications/resources/updated")
        );
        assert_eq!(messages[0]["params"]["uri"], json!(uri));
        assert_eq!(
            messages[0]["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
            json!("listen-1")
        );
        // 同一个 revision 再拍一次：只发一次（水位已经推上去）。
        assert!(subscription.updates(&tick(1, &[(uri, 3)])).is_empty());

        // 会话关了：资源集合变了 → list_changed。
        let messages = subscription.updates(&tick(2, &[]));
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0]["method"],
            json!("notifications/resources/list_changed")
        );

        // 客户端没勾 listChanged 就不发它（规范 MUST NOT：没勾的类型一条都不许发）。
        let mut quiet = accept(
            &request(json!({ "resourceSubscriptions": [uri] })),
            &tick(1, &[(uri, 0)]),
        )
        .expect("接受");
        assert!(quiet.updates(&tick(1, &[(uri, 0)])).is_empty());
        assert!(quiet.updates(&tick(2, &[])).is_empty());
        assert_eq!(quiet.updates(&tick(2, &[(uri, 1)])).len(), 1);
    }

    #[test]
    fn a_resource_that_appears_after_the_baseline_is_aligned_not_announced() {
        let uri = "vox://session/s_a/transcript";
        // 基线里没有这条 URI（客户端先订阅、后开会话）。
        let mut subscription = accept(
            &request(json!({"resourceSubscriptions": [uri]})),
            &tick(0, &[]),
        )
        .expect("接受");
        // 它第一次露面：只对齐水位，不补 updated（那条会话的字客户端一个字都没见过）。
        assert!(subscription.updates(&tick(1, &[(uri, 5)])).is_empty());
        // 之后真变了：照发。
        assert_eq!(subscription.updates(&tick(1, &[(uri, 6)])).len(), 1);
    }

    #[test]
    fn only_subscribed_uris_are_reported() {
        let mine = "vox://session/s_a/transcript";
        let other = "vox://session/s_b/transcript";
        let mut subscription = accept(
            &request(json!({ "resourceSubscriptions": [mine] })),
            &tick(1, &[(mine, 0), (other, 0)]),
        )
        .expect("接受");
        assert!(subscription
            .updates(&tick(1, &[(mine, 0), (other, 0)]))
            .is_empty());

        let messages = subscription.updates(&tick(1, &[(mine, 0), (other, 7)]));
        assert!(messages.is_empty(), "别人订阅的 URI 与我无关：{messages:?}");

        let messages = subscription.updates(&tick(1, &[(mine, 1), (other, 7)]));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["params"]["uri"], json!(mine));
    }

    #[test]
    fn closing_carries_the_subscription_id_and_a_complete_result() {
        let subscription = accept(&request(json!({})), &tick(0, &[])).expect("接受");
        let closed = subscription.closed();
        assert_eq!(closed["id"], json!("listen-1"));
        assert_eq!(closed["result"]["resultType"], json!("complete"));
        assert_eq!(
            closed["result"]["_meta"][meta::key::SUBSCRIPTION_ID],
            json!("listen-1")
        );
    }
}
