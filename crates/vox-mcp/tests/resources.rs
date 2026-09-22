//! 资源面用例（`resources.rs` + `transcript.rs` + `mcp/{resources,subscriptions}.rs`）。
//!
//! 全部走**真 `Runtime` + 真 `LedgerBackend` + 公开协议面**（[`vox_mcp::handle`]）：字幕不是造的，
//! 是芯的字幕轨道真收了一条 delta 之后、账本真算出来的那一份。断言的是外部可见的契约——
//! `resources/list` 列什么、快照有哪些格、`ttlMs`/`cacheScope` 定值、资源不存在是 `-32602`、
//! 订阅先 ack 再按水位发通知、**广告出去的位必须真有对应的通知**（"位 = 事实"）。
//!
//! 传输面（真 socket + SSE）在 `tests/lifecycle.rs` 里端到端跑一遍。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use vox_core::capability::HostFacts;
use vox_core::composition::HostKind;
use vox_core::event::{Pipeline, PipelineState};
use vox_core::ports::{Clock, PortResult};
use vox_core::runtime::{PipelineCommand, PipelineControl, Runtime};
use vox_core::settings::{ListenTarget, ModelProvider, Settings};
use vox_core::usage::Stamp;

use vox_mcp::handlers::ControlBackend;
use vox_mcp::jsonrpc::code;
use vox_mcp::mcp::subscriptions::Subscription;
use vox_mcp::mcp::{self, meta};
use vox_mcp::session::LedgerBackend;
use vox_mcp::{Answer, EndpointId, Grants, Permission};

/// 可控时钟：`updated_at_ms` 与"两句话之间隔了多久"都由它说了算（真等不现实）。
#[derive(Default)]
struct TestClock(AtomicU64);

impl TestClock {
    fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    fn stamp(&self) -> Stamp {
        Stamp {
            unix_secs: 0,
            year: 2026,
            month: 9,
            day: 22,
        }
    }
}

/// 桌面档、一个位都不关（与 `tests/endpoints.rs` 同一条口径）。
fn facts() -> HostFacts {
    HostFacts {
        host: HostKind::Windows,
        off: BTreeMap::new(),
        virtual_mic_device: None,
    }
}

/// 两条腿都配好（`listen` 没目标就派不出清单），控制面打开、去抖间隔 250 ms。
fn settings() -> Settings {
    let mut settings = Settings::default();
    settings.speak.input_device = Some("Yeti Stereo Microphone".to_string());
    settings.speak.output_device = Some("CABLE Input (VB-Audio Virtual Cable)".to_string());
    settings.listen.target = Some(ListenTarget {
        executable: "Discord.exe".to_string(),
        display_name: "Discord".to_string(),
        include_process_tree: true,
    });
    settings.control.enabled = true;
    settings.control.allow_microphone = true;
    settings.control.allow_system_audio = true;
    settings.control.allow_audible_output = true;
    settings.control.transcript_notify_ms = 250;
    settings
}

/// 全开的授权位（对照用的假实现；真实现是 `impl Grants for Runtime`）。
struct Granted;

impl Grants for Granted {
    fn user_granted(&self, _permission: Permission) -> bool {
        true
    }

    fn config_write_allowed(&self) -> bool {
        true
    }
}

/// 假流水线控制面：接到 `Start` 立刻报 `Ready`，**并且记下芯分配的那个会话号**——
/// 用例要拿它去喂 `on_subtitle_delta`（芯会丢掉会话号对不上的字幕，这是它的规矩）。
struct ReadyControl {
    runtime: Runtime,
    started: Mutex<Vec<(Pipeline, u64)>>,
}

impl ReadyControl {
    fn session_id(&self, pipeline: Pipeline) -> u64 {
        self.started
            .lock()
            .expect("锁")
            .iter()
            .rev()
            .find(|(started, _)| *started == pipeline)
            .map(|(_, session_id)| *session_id)
            .expect("这条流水线起过")
    }
}

impl PipelineControl for ReadyControl {
    fn apply(&self, command: PipelineCommand) -> PortResult<()> {
        if let PipelineCommand::Start(config) = command {
            self.started
                .lock()
                .expect("锁")
                .push((config.pipeline, config.session_id));
            self.runtime.on_pipeline_state(
                config.pipeline,
                config.session_id,
                PipelineState::Ready,
            );
        }
        Ok(())
    }
}

/// 一套跑得起来的本机：真账本 + 真后端 + 可控时钟 + 能喂字幕的假控制面。
struct Harness {
    runtime: Runtime,
    clock: Arc<TestClock>,
    control: Arc<ReadyControl>,
    backend: LedgerBackend<Runtime, Granted>,
}

fn harness() -> Harness {
    let clock = Arc::new(TestClock::default());
    let runtime = Runtime::new(settings(), clock.clone());
    runtime.set_host_facts(facts());
    runtime.set_api_key_for(ModelProvider::Aliyun, "test-key");
    let control = Arc::new(ReadyControl {
        runtime: runtime.clone(),
        started: Mutex::new(Vec::new()),
    });
    runtime.set_control(control.clone());
    let backend = LedgerBackend::new(runtime.clone(), Granted);
    Harness {
        runtime,
        clock,
        control,
        backend,
    }
}

impl Harness {
    /// 开一条会话，返回 handle。
    fn open(&mut self, endpoint: EndpointId) -> String {
        let opened = self
            .backend
            .session_open(endpoint, 5_000)
            .expect("session_open");
        opened["session"].as_str().expect("handle").to_string()
    }

    /// 喂一条字幕 delta（走芯的公开面，和真实转写走的是同一条路）。
    ///
    /// `delta` 是**这一条事件带的新字**（不是整句）：芯的 `SubtitleTrack::push_text` 是往后追加的，
    /// 整句累积是流水线那侧算增量之后的事。`confirmed` 是服务端说"这段不再变了"的整句前缀。
    fn speak(&self, endpoint: EndpointId, delta: &str, confirmed: &str, done: bool) {
        let pipeline = match endpoint {
            EndpointId::Speak => Pipeline::Speak,
            EndpointId::Listen => Pipeline::Listen,
        };
        self.runtime.on_subtitle_delta(
            pipeline,
            self.control.session_id(pipeline),
            delta,
            done,
            false,
            Some(confirmed),
        );
    }

    /// 一次协议调用（`handle` 是三个出口共用的那一个函数）。
    fn call(&mut self, id: i64, method: &str, params: Value) -> Value {
        let mut params = params.as_object().cloned().unwrap_or_default();
        params.insert("_meta".to_string(), meta_object());
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        mcp::handle(&message, Some(&mut self.backend))
            .response()
            .unwrap_or_else(|| panic!("{method} 必须回一条响应"))
    }

    /// 一条 `subscriptions/listen`：接受就是一条流（不是响应）。
    fn listen(&mut self, id: i64, notifications: Value) -> Subscription {
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "subscriptions/listen",
            "params": { "_meta": meta_object(), "notifications": notifications },
        });
        match mcp::handle(&message, Some(&mut self.backend)) {
            Answer::Stream(subscription) => subscription,
            other => panic!("listen 该被接受成一条流，拿到 {other:?}"),
        }
    }

    /// ticker 的一拍（传输面每 `notify_ms` 调一次的就是它）。
    fn tick(&mut self) -> vox_mcp::resources::ResourceTick {
        self.backend.poll_resources()
    }

    /// 读一条资源的快照（`contents[0].text` 解析出来）。
    fn read(&mut self, id: i64, uri: &str) -> Value {
        let response = self.call(id, "resources/read", json!({ "uri": uri }));
        let text = response["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("contents[0].text 必须是快照 JSON：{response}"))
            .to_string();
        serde_json::from_str(&text).expect("快照是合法 JSON")
    }

    fn uri(handle: &str) -> String {
        format!("vox://session/{handle}/transcript")
    }
}

fn meta_object() -> Value {
    json!({
        meta::key::PROTOCOL_VERSION: meta::PROTOCOL_VERSION,
        meta::key::CLIENT_INFO: { "name": "resources-test", "version": "0" },
        meta::key::CLIENT_CAPABILITIES: {},
    })
}

fn error_code(response: &Value) -> i64 {
    response["error"]["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("这不是错误响应：{response}"))
}

// --- resources/list ----------------------------------------------------------------

#[test]
fn resources_list_shows_one_transcript_per_live_session() {
    let mut harness = harness();

    // 一条会话都没开：空数组，不是错误（规范：集合 MAY 为空）。
    let listed = harness.call(1, "resources/list", json!({}));
    assert_eq!(listed["result"]["resources"], json!([]));
    assert_eq!(listed["result"]["resultType"], json!("complete"));
    // 缓存提示必带：会话随时开/关，而且是用户私有信息。
    assert_eq!(listed["result"]["ttlMs"], json!(0));
    assert_eq!(listed["result"]["cacheScope"], json!("private"));

    let speak = harness.open(EndpointId::Speak);
    let listen = harness.open(EndpointId::Listen);

    let listed = harness.call(2, "resources/list", json!({}));
    let resources = listed["result"]["resources"]
        .as_array()
        .expect("resources 是数组");
    assert_eq!(resources.len(), 2, "{listed}");
    assert_eq!(resources[0]["uri"], json!(Harness::uri(&speak)));
    assert_eq!(resources[0]["name"], json!("transcript"));
    assert_eq!(resources[0]["mimeType"], json!("application/json"));
    assert_eq!(resources[0]["title"], json!("对外说话 · 字幕"));
    assert_eq!(resources[1]["uri"], json!(Harness::uri(&listen)));
    assert_eq!(resources[1]["title"], json!("听人说话 · 字幕"));

    // 关掉一条 → 它从列表里消失（设计稿 §2.4.1：`session_close` 后资源消失）。
    harness
        .backend
        .session_close(&speak)
        .expect("session_close");
    let listed = harness.call(3, "resources/list", json!({}));
    let resources = listed["result"]["resources"]
        .as_array()
        .expect("resources 是数组");
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["uri"], json!(Harness::uri(&listen)));
}

// --- resources/read ----------------------------------------------------------------

#[test]
fn resources_read_returns_a_snapshot_that_moves_with_the_subtitles() {
    let mut harness = harness();
    let handle = harness.open(EndpointId::Speak);
    let uri = Harness::uri(&handle);

    // 还没说话：快照存在、text 空、revision 从 0 起。
    let response = harness.call(1, "resources/read", json!({ "uri": uri }));
    assert_eq!(response["result"]["resultType"], json!("complete"));
    assert_eq!(
        response["result"]["contents"][0]["mimeType"],
        json!("application/json")
    );
    assert_eq!(response["result"]["contents"][0]["uri"], json!(uri));
    // 缓存提示必带，且**不许缓存住旧字幕**（0 = 立刻陈旧）。
    assert_eq!(response["result"]["ttlMs"], json!(0));
    assert_eq!(response["result"]["cacheScope"], json!("private"));

    let snapshot = harness.read(2, &uri);
    assert_eq!(snapshot["session"], json!(handle));
    assert_eq!(snapshot["endpoint"], json!("speak"));
    assert_eq!(snapshot["track"], json!("speak"));
    assert_eq!(snapshot["state"], json!("ready"));
    assert_eq!(snapshot["text"], json!(""));
    assert_eq!(snapshot["revision"], json!(0));
    assert_eq!(snapshot["notify_ms"], json!(250));
    assert_eq!(snapshot["confirmed"], Value::Null);
    assert_eq!(snapshot["last_delta_done"], json!(false));

    // 说一句话：text 出现、confirmed 跟上、revision 变大（验收 §4-15：隔一会儿再读 revision 变大）。
    harness.clock.set(1_000);
    harness.speak(EndpointId::Speak, "Hello, nice", "Hello, nice", false);
    let snapshot = harness.read(3, &uri);
    assert_eq!(snapshot["text"], json!("Hello, nice"));
    assert_eq!(snapshot["confirmed"], json!("Hello, nice"));
    assert_eq!(snapshot["last_delta_done"], json!(false));
    assert_eq!(snapshot["revision"], json!(1), "变了就要涨：{snapshot}");
    assert_eq!(snapshot["updated_at_ms"], json!(1_000));

    // 同一句话再读一次：没变 → revision 不动（客户端靠它判断"有没有漏掉通知"）。
    let again = harness.read(4, &uri);
    assert_eq!(again["revision"], json!(1));

    // 又说了半句 + 这一段说完了：text/confirmed/last_delta_done 一起动。
    harness.clock.set(2_000);
    harness.speak(
        EndpointId::Speak,
        " to meet you",
        "Hello, nice to meet you",
        true,
    );
    let snapshot = harness.read(5, &uri);
    assert_eq!(snapshot["text"], json!("Hello, nice to meet you"));
    assert_eq!(snapshot["confirmed"], json!("Hello, nice to meet you"));
    assert_eq!(snapshot["last_delta_done"], json!(true));
    assert_eq!(snapshot["revision"], json!(2));
    assert_eq!(snapshot["updated_at_ms"], json!(2_000));
}

#[test]
fn resources_read_rejects_unknown_and_closed_uris_with_invalid_params() {
    let mut harness = harness();
    let handle = harness.open(EndpointId::Speak);
    let uri = Harness::uri(&handle);

    // 形状不对 / 从没签发过的 handle / 别人的 URI：一律 `-32602`（规范 MUST，不是旧的 -32002），
    // 而且**不许**回一个空的 `contents` 数组（规范：空数组有歧义）。
    for (what, unknown) in [
        (
            "不是我们签发的 URI",
            "file:///project/config.json".to_string(),
        ),
        ("形状差一点", format!("vox://session/{handle}")),
        ("handle 是空的", "vox://session//transcript".to_string()),
        ("从没签发过的 handle", Harness::uri("s_0000000000000000")),
    ] {
        let response = harness.call(1, "resources/read", json!({ "uri": unknown }));
        assert_eq!(error_code(&response), -32602, "{what}：{response}");
        assert_eq!(response["error"]["data"]["uri"], json!(unknown));
        assert!(response.get("result").is_none(), "{what} 不许带 result");
    }

    // 缺 `uri` 也是 `-32602`（参数不满足 schema）。
    let missing = harness.call(2, "resources/read", json!({}));
    assert_eq!(error_code(&missing), -32602);
    assert_eq!(missing["error"]["data"]["path"], json!("uri"));

    // 关掉的会话：资源没了（验收 §4-17）。
    harness.backend.session_close(&handle).expect("close");
    let closed = harness.call(3, "resources/read", json!({ "uri": uri }));
    assert_eq!(error_code(&closed), -32602, "{closed}");
}

// --- subscriptions/listen ----------------------------------------------------------

#[test]
fn a_subscription_acknowledges_then_reports_updates_and_list_changes() {
    let mut harness = harness();
    let handle = harness.open(EndpointId::Speak);
    let uri = Harness::uri(&handle);

    let mut subscription = harness.listen(
        7,
        json!({
            "toolsListChanged": true,
            "resourcesListChanged": true,
            "resourceSubscriptions": [uri, "file:///project/config.json"],
        }),
    );

    // 第一条**必须**是 ack，而且只勾我们真会发的两类（规范 MUST：第一条 + 只报同意的部分）。
    let acknowledged = subscription.acknowledged();
    assert_eq!(
        acknowledged["method"],
        json!("notifications/subscriptions/acknowledged")
    );
    assert_eq!(
        acknowledged["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
        json!(7),
        "subscriptionId = listen 请求的 JSON-RPC id"
    );
    assert_eq!(
        acknowledged["params"]["notifications"],
        json!({ "resourcesListChanged": true, "resourceSubscriptions": [uri] })
    );

    // 订阅那一刻就对好了水位（`mcp::listen` 拿的这一拍）：刚订阅上来不为"它本来就有字"发通知。
    assert!(subscription.updates(&harness.tick()).is_empty());

    // 说一句话 → 一拍之后收到 `notifications/resources/updated`。
    harness.clock.set(1_000);
    harness.speak(EndpointId::Speak, "你好", "你好", false);
    let messages = subscription.updates(&harness.tick());
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(
        messages[0]["method"],
        json!("notifications/resources/updated")
    );
    assert_eq!(messages[0]["params"]["uri"], json!(uri));
    assert_eq!(
        messages[0]["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
        json!(7)
    );

    // 收到通知之后真能读到新字幕（客户端该做的事：重读快照）。
    let snapshot = harness.read(1, &uri);
    assert_eq!(snapshot["text"], json!("你好"));
    assert_eq!(snapshot["revision"], json!(1));

    // 没变的一拍不发（去抖：逐 token 的 delta 不该变成逐 token 的通知）。
    assert!(subscription.updates(&harness.tick()).is_empty());

    // **读过的资源不再重复发**：`resources/read` 也会观察、也会涨 revision，
    // 但订阅流比的是自己发到哪的水位，所以不会因为"别人读过"而漏掉或重发。
    harness.clock.set(2_000);
    harness.speak(
        EndpointId::Speak,
        "，很高兴认识你",
        "你好，很高兴认识你",
        false,
    );
    let snapshot = harness.read(2, &uri);
    assert_eq!(snapshot["revision"], json!(2), "读也观察：{snapshot}");
    let messages = subscription.updates(&harness.tick());
    assert_eq!(messages.len(), 1, "水位落后就该补上：{messages:?}");
    assert_eq!(messages[0]["params"]["uri"], json!(uri));
    assert!(subscription.updates(&harness.tick()).is_empty());

    // 关掉会话：资源集合变了 → `notifications/resources/list_changed`。
    harness.backend.session_close(&handle).expect("close");
    let messages = subscription.updates(&harness.tick());
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(
        messages[0]["method"],
        json!("notifications/resources/list_changed")
    );
    assert_eq!(
        messages[0]["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
        json!(7)
    );

    // 再读同一 URI → `-32602`（验收 §4-17）。
    let gone = harness.call(3, "resources/read", json!({ "uri": uri }));
    assert_eq!(error_code(&gone), -32602);
}

#[test]
fn a_subscription_only_gets_what_it_asked_for() {
    let mut harness = harness();
    let handle = harness.open(EndpointId::Speak);
    let uri = Harness::uri(&handle);

    // 只勾"字幕变更"、不勾"列表变化"：会话开关时**一条都不许发**（规范 MUST NOT）。
    let mut quiet = harness.listen(1, json!({ "resourceSubscriptions": [uri] }));
    assert!(quiet.updates(&harness.tick()).is_empty());
    harness.clock.set(1_000);
    harness.speak(EndpointId::Speak, "甲", "甲", false);
    assert_eq!(quiet.updates(&harness.tick()).len(), 1);
    harness.backend.session_close(&handle).expect("close");
    assert!(
        quiet.updates(&harness.tick()).is_empty(),
        "没勾 listChanged"
    );

    // 什么都不勾：连 ack 之后一条都不发（同意的那份是空对象）。
    let mut nothing = harness.listen(2, json!({}));
    assert_eq!(nothing.acknowledged()["params"]["notifications"], json!({}));
    assert!(nothing.updates(&harness.tick()).is_empty());
    assert!(nothing.updates(&harness.tick()).is_empty());
}

#[test]
fn subscribing_before_the_session_opens_is_accepted_and_silent_until_it_does() {
    let mut harness = harness();
    // 客户端先订阅、后开会话（设计稿 §2.4.2：接受，在那之前不发任何东西）。
    let uri = Harness::uri("s_0123456789abcdef");
    let mut subscription = harness.listen(
        1,
        json!({ "resourcesListChanged": true, "resourceSubscriptions": [uri] }),
    );
    assert!(subscription.updates(&harness.tick()).is_empty());

    let handle = harness.open(EndpointId::Speak);
    assert_ne!(
        Harness::uri(&handle),
        uri,
        "handle 是服务端签发的，猜不出来"
    );
    // 会话开了 = 列表变了 → list_changed；那条"还没开的"URI 仍然一条不发。
    let messages = subscription.updates(&harness.tick());
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["method"],
        json!("notifications/resources/list_changed")
    );
}

#[test]
fn listen_without_a_backend_or_with_a_bad_filter_is_refused() {
    // 没有后端：不接受订阅（ticker 没有账本可读，那条流会永远一条都不响——那是假订阅）。
    let message = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "subscriptions/listen",
        "params": { "_meta": meta_object(), "notifications": {} },
    });
    match mcp::handle(&message, None) {
        Answer::Response(response) => {
            assert_eq!(error_code(&response), i64::from(code::INTERNAL_ERROR));
            assert!(response["error"]["message"]
                .as_str()
                .expect("message")
                .contains("后端"));
        }
        other => panic!("没有后端时不该开流：{other:?}"),
    }

    // 有后端但过滤写坏了：`-32602`，也不是开流。
    let mut harness = harness();
    let broken = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "subscriptions/listen",
        "params": {
            "_meta": meta_object(),
            "notifications": { "resourceSubscriptions": "vox://session/s_a/transcript" },
        },
    });
    match mcp::handle(&broken, Some(&mut harness.backend)) {
        Answer::Response(response) => assert_eq!(error_code(&response), -32602),
        other => panic!("过滤写坏了不该开流：{other:?}"),
    }

    // 资源面在没后端时也是 `-32603`，不假装"一条资源都没有"。
    for method in ["resources/list", "resources/read"] {
        let params = if method == "resources/read" {
            json!({ "uri": "vox://session/s_a/transcript" })
        } else {
            json!({})
        };
        let mut params = params.as_object().cloned().expect("对象");
        params.insert("_meta".to_string(), meta_object());
        let message = json!({ "jsonrpc": "2.0", "id": 3, "method": method, "params": params });
        let response = mcp::handle(&message, None).response().expect("必须有响应");
        assert_eq!(
            error_code(&response),
            i64::from(code::INTERNAL_ERROR),
            "{method}"
        );
    }
}

// --- 位 = 事实 ---------------------------------------------------------------------

/// 广告出去的每一位都必须真有对应的通知——这条用例是"位必须是事实"的机械证明：
/// 它不检查文档、不检查注释，只检查**真发出去的消息**。
#[test]
fn every_advertised_capability_bit_has_a_real_notification() {
    let mut harness = harness();
    let discovered = harness.call(1, "server/discover", json!({}));
    let capabilities = &discovered["result"]["capabilities"];

    // `resources.subscribe` → `notifications/resources/updated`。
    assert_eq!(capabilities["resources"]["subscribe"], json!(true));
    // `resources.listChanged` → `notifications/resources/list_changed`。
    assert_eq!(capabilities["resources"]["listChanged"], json!(true));

    let handle = harness.open(EndpointId::Speak);
    let uri = Harness::uri(&handle);
    let mut subscription = harness.listen(
        1,
        json!({ "resourcesListChanged": true, "resourceSubscriptions": [uri] }),
    );
    assert!(subscription.updates(&harness.tick()).is_empty());

    // ① subscribe 位承诺的那条通知。
    harness.clock.set(1_000);
    harness.speak(EndpointId::Speak, "位必须是事实", "位必须是事实", false);
    let updates = subscription.updates(&harness.tick());
    assert_eq!(
        updates[0]["method"],
        json!("notifications/resources/updated"),
        "`resources.subscribe: true` 承诺的就是它"
    );

    // ② listChanged 位承诺的那条通知。
    harness.backend.session_close(&handle).expect("close");
    let changes = subscription.updates(&harness.tick());
    assert_eq!(
        changes[0]["method"],
        json!("notifications/resources/list_changed"),
        "`resources.listChanged: true` 承诺的就是它"
    );

    // ③ 没广告的位一条都不许发：`tools.listChanged` / `prompts` / `extensions` 都不在能力里，
    //    所以 `notifications/tools/list_changed` 这类消息在整个资源面上一次都不出现。
    for bit in ["tools", "prompts", "extensions"] {
        if bit == "tools" {
            assert!(
                capabilities["tools"].get("listChanged").is_none(),
                "5 个工具恒定，不许广告 tools.listChanged：{capabilities}"
            );
        } else {
            assert!(
                capabilities.get(bit).is_none(),
                "{bit} 还没实现，就不许广告：{capabilities}"
            );
        }
    }
    let all: Vec<String> = updates
        .iter()
        .chain(changes.iter())
        .map(|message| message["method"].to_string())
        .collect();
    assert!(
        !all.iter()
            .any(|method| method.contains("tools/list_changed")),
        "没广告的位一条都不许发：{all:?}"
    );
}

/// 订阅**之后**发生的第一次变化必须被通知到——哪怕它正好落在"ack 与 ticker 第一拍"之间。
///
/// 这条窗口真会踩中：客户端拿到 ack 之后马上说话，而 ticker 的第一次观察可能还没跑（高负载下
/// ticker 线程的唤醒会被压后）。基线一旦拖到那一拍才登记，这次变化就**永远**不通知——水位与
/// 变更检测的指纹都停在这一刻，之后状态稳定，没有第二次机会。`tests/lifecycle.rs` 那条 SSE
/// 端到端用例偶发红就是这个（实测跑到 30 s 上限也没等到通知）。
/// 钉住的是外部可见的契约：**订阅之后变的东西，订阅者收得到**。
#[test]
fn a_change_that_lands_right_after_subscribing_is_reported() {
    let mut harness = harness();
    let handle = harness.open(EndpointId::Speak);
    let uri = Harness::uri(&handle);
    let mut subscription = harness.listen(1, json!({ "resourceSubscriptions": [uri] }));

    // 客户端拿到 ack 之后立刻说了一句——这条 diff 落在"订阅"与"ticker 第一拍"之间。
    harness.clock.set(1_000);
    harness.speak(EndpointId::Speak, "你好", "你好", false);

    let messages = subscription.updates(&harness.tick());
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(
        messages[0]["method"],
        json!("notifications/resources/updated")
    );
    assert_eq!(messages[0]["params"]["uri"], json!(uri));
}
