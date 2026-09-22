//! 端到端（本机 loopback）：真 `serve` + 真账本后端 + 真 socket。
//!
//! 这一条证明的不是"函数能跑"，而是**外部 Agent 能调通**：一个独立进程按 MCP 2026-07-28 的
//! 规矩（每请求 `_meta`、`MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` 三个头、Bearer token）
//! 打 `tools/call`，拿到的是账本真派生出来的清单——投影、反方向表、两段式确认、会话生命周期
//! 全在同一条线上。
//!
//! 跑法（要看线上的原始字节就加 `--nocapture`）：
//!
//! ```text
//! cargo test -p vox-mcp --test lifecycle -- --nocapture
//! ```
//!
//! **长流的等待上限是可配的**（[`event_wait`]，默认 30 s；`VOX_MCP_TEST_EVENT_TIMEOUT_MS` 覆写）：
//! 这条流上的每一拍都要等真 socket 上的字节，写死一个很短的上限（曾用 5 s）在高负载下会红。
//! 放大上限**不够**——高负载复跑的原始症状是等满 30 s 也没等到那条通知，查下去是产品侧一条
//! 窗口：订阅后的第一次变化被"拖到第一拍才对齐的水位"吞掉（基线现在在回 ack 之前取，见
//! `mcp::subscriptions::accept` 的注释与 `tests/resources.rs` 的回归用例）。这里留下的
//! "上限 + 存活判定"是安全网：调度真会把一拍压到几秒之外，但那是慢，不是坏。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use vox_core::capability::HostFacts;
use vox_core::composition::HostKind;
use vox_core::event::{Pipeline, PipelineState};
use vox_core::ports::{Clock, PortResult};
use vox_core::runtime::{PipelineCommand, PipelineControl, Runtime};
use vox_core::settings::{ListenTarget, ModelProvider, Settings};
use vox_core::usage::Stamp;

use vox_mcp::ledger::Ledger;
use vox_mcp::mcp::meta;
use vox_mcp::session::LedgerBackend;
use vox_mcp::transport::http::{ServerOptions, PATH};
use vox_mcp::{serve, BoxedBackend};

/// 等一条 SSE 事件 / 一行长流的**上限**（[`Agent::next_event`] 的 `cap`、[`Agent::connect`] 的
/// 读超时）。默认 30 s：这是**高负载下曾偶发**之后放宽的——本机跟一次 `cargo build` 并发跑时，
/// 原先写死的 5 s 不够。上限只治"慢"那一种；复跑里另一种是**通知真丢了**（订阅后的第一次变化
/// 被吞，已在 `mcp::subscriptions::accept` 修掉），那种放大多少秒都等不到。
/// 要更快看到红，用 `VOX_MCP_TEST_EVENT_TIMEOUT_MS` 覆写。
const EVENT_WAIT_MS_DEFAULT: u64 = 30_000;

/// 每读一次长流的等待切片：比 [`event_wait`] 的上限短，段与段之间问一句流还活着没有——把
/// "对端慢慢来"与"对端已经断了"分成两件事（断了的可以立刻报，慢了接着等）。
const EVENT_POLL_SLICE: Duration = Duration::from_millis(500);

fn event_wait() -> Duration {
    Duration::from_millis(
        std::env::var("VOX_MCP_TEST_EVENT_TIMEOUT_MS")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(EVENT_WAIT_MS_DEFAULT),
    )
}

/// [`Agent::poll`] 的一拍结果。
enum Poll {
    /// 读到新字节（已经收进 `inbox`）。
    Bytes,
    /// 这一拍没数据，但对端还开着（读超时）。
    Idle,
    /// 对端关了连接。
    Closed,
}

/// 这个 `io::Error` 是"读超时"（`SO_RCVTIMEO` 到点）：Linux 上是 `WouldBlock`。
fn is_read_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

#[derive(Default)]
struct TestClock(AtomicU64);

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

/// 接到 `Start` 立刻报 `Ready` 的假控制面：真起设备不在这条用例的射程里（那是媒体面的事）。
///
/// 顺手记下芯分配的那个**会话号**：字幕要喂回账本（`on_subtitle_delta`）就得带上它，
/// 芯会丢掉会话号对不上的字幕（那是它的规矩）。
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

/// 一套本机：真账本 + 真后端 + 能喂字幕的假控制面。用例拿 `runtime` 说话、拿 `backend` 起服务。
struct Fixture {
    runtime: Runtime,
    control: Arc<ReadyControl>,
    backend: BoxedBackend,
}

/// 喂一条字幕 delta（`delta` = 这一条事件带的新字，`confirmed` = 不再会变的整句前缀）。
///
/// 走的是芯的公开面 `Runtime::on_subtitle_delta`，和真实转写进来的是同一条路——会话号对不上
/// 或这条腿没开字幕时它会**丢掉**这条 delta，正是我们要验的那条规矩。
fn speak(
    runtime: &Runtime,
    control: &ReadyControl,
    pipeline: Pipeline,
    delta: &str,
    confirmed: &str,
    done: bool,
) {
    runtime.on_subtitle_delta(
        pipeline,
        control.session_id(pipeline),
        delta,
        done,
        false,
        Some(confirmed),
    );
}

/// 装配层将来要接的那一份：真 `Runtime` + 注入的事实 → `LedgerBackend` → `serve`。
///
/// **授权位就是设置里那四格**（`LedgerBackend::new(runtime.clone(), runtime)`，`impl Grants for
/// Runtime`）：用例在 `Settings.control` 里把位拨开，跟用户在界面上拨的是同一处。
fn fixture() -> Fixture {
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
    settings.control.allow_config_write = true;
    let runtime = Runtime::new(settings, Arc::new(TestClock::default()));
    runtime.set_host_facts(HostFacts {
        host: HostKind::Windows,
        off: BTreeMap::new(),
        virtual_mic_device: None,
    });
    runtime.set_api_key_for(ModelProvider::Aliyun, "test-key");
    let control = Arc::new(ReadyControl {
        runtime: runtime.clone(),
        started: Mutex::new(Vec::new()),
    });
    runtime.set_control(control.clone());
    let backend: BoxedBackend = Box::new(LedgerBackend::new(runtime.clone(), runtime.clone()));
    Fixture {
        runtime,
        control,
        backend,
    }
}

fn options(name: &str) -> ServerOptions {
    let path = std::env::temp_dir()
        .join(format!("vox-mcp-lifecycle-{}-{name}", std::process::id()))
        .join("control.json");
    let _ = std::fs::remove_dir_all(path.parent().expect("父目录"));
    ServerOptions::new(path)
}

/// 裸 HTTP 客户端：一个外部 Agent 的最小形态（三个 MCP 头 + Bearer token）。
struct Agent {
    stream: TcpStream,
    inbox: Vec<u8>,
    token: String,
    /// 上一条响应的状态码（负例要看它：协议错误在 HTTP 上是 400，不是 200）。
    status: u16,
}

impl Agent {
    fn connect(addr: SocketAddr, token: &str) -> Self {
        let stream = TcpStream::connect(addr).expect("连服务端");
        stream
            .set_read_timeout(Some(event_wait()))
            .expect("设读超时");
        Self {
            stream,
            inbox: Vec::new(),
            token: token.to_string(),
            status: 0,
        }
    }

    /// 一条 `tools/call`。
    fn call(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
            Some(name),
        )
    }

    /// 任意一条请求。`name` = `Mcp-Name` 头的值（规范要求 `tools/call` 与 `resources/read`
    /// 必带，其余方法不带这个头）。
    fn request(&mut self, id: i64, method: &str, params: Value, name: Option<&str>) -> Value {
        let mut params = params.as_object().cloned().unwrap_or_default();
        params.insert("_meta".to_string(), self.meta());
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string();
        let name_header = name.map_or(String::new(), |name| format!("Mcp-Name: {name}\r\n"));
        let request = format!(
            "POST {PATH} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n\
             MCP-Protocol-Version: {version}\r\n\
             Mcp-Method: {method}\r\n\
             {name_header}\
             Authorization: Bearer {token}\r\n\
             Content-Length: {length}\r\n\r\n{body}",
            version = meta::PROTOCOL_VERSION,
            token = self.token,
            length = body.len(),
        );
        println!("--- 请求（{method}）\n{request}");
        self.stream
            .write_all(request.as_bytes())
            .expect("写请求（服务端可能提前关了连接）");
        let response = self.read();
        println!("--- 响应（{method}）[HTTP {}]\n{response}\n", self.status);
        serde_json::from_str(&response).expect("响应必须是 JSON")
    }

    fn read(&mut self) -> String {
        let end = loop {
            if let Some(end) = find(&self.inbox, b"\r\n\r\n") {
                break end;
            }
            let mut chunk = [0u8; 4096];
            let read = self.stream.read(&mut chunk).expect("读响应头");
            assert!(read > 0, "服务端在回完整响应前关了连接");
            self.inbox.extend_from_slice(&chunk[..read]);
        };
        let head = String::from_utf8(self.inbox.drain(..end + 4).collect()).expect("头是 UTF-8");
        let length: usize = head
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse().expect("Content-Length 是数字"))
            .unwrap_or(0);
        while self.inbox.len() < length {
            let mut chunk = [0u8; 4096];
            let read = self.stream.read(&mut chunk).expect("读响应体");
            assert!(read > 0, "服务端在回完整响应体前关了连接");
            self.inbox.extend_from_slice(&chunk[..read]);
        }
        self.status = head
            .split(' ')
            .nth(1)
            .expect("状态码")
            .parse()
            .expect("状态码是数字");
        String::from_utf8(self.inbox.drain(..length).collect()).expect("体是 UTF-8")
    }

    /// 开一条 `subscriptions/listen` 长流：写请求 → 读 SSE 头 → 读第一条消息（规范 MUST：ack）。
    ///
    /// 长流**没有 `Content-Length`**（靠关连接划界），所以这里只读到 `\r\n\r\n` 为止，
    /// 之后的事件由 [`Agent::next_event`] 一条条取。
    fn listen(&mut self, id: i64, notifications: Value) -> Value {
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "subscriptions/listen",
            "params": { "_meta": self.meta(), "notifications": notifications },
        })
        .to_string();
        let request = format!(
            "POST {PATH} HTTP/1.1\r\n\
             Host: 127.0.0.1\r\n\
             Content-Type: application/json\r\n\
             Accept: text/event-stream\r\n\
             MCP-Protocol-Version: {version}\r\n\
             Mcp-Method: subscriptions/listen\r\n\
             Authorization: Bearer {token}\r\n\
             Content-Length: {length}\r\n\r\n{body}",
            version = meta::PROTOCOL_VERSION,
            token = self.token,
            length = body.len(),
        );
        println!("--- 请求（subscriptions/listen）\n{request}");
        self.stream.write_all(request.as_bytes()).expect("写请求");
        let head = self.read_head();
        println!("--- 响应头\n{head}");
        assert!(
            head.starts_with("HTTP/1.1 200 OK"),
            "长流必须是 200：{head}"
        );
        assert!(
            head.to_lowercase()
                .contains("content-type: text/event-stream"),
            "长流必须是 text/event-stream：{head}"
        );
        assert!(
            head.contains("X-Accel-Buffering: no"),
            "规范 SHOULD：起流要带 X-Accel-Buffering: no：{head}"
        );
        self.next_event(event_wait())
    }

    /// 读一条 SSE 事件（`data: <json>`）；`:` 开头的保活注释行跳过并打印。
    ///
    /// `cap` 是**这次等待的总上限**，不是"读一次的超时"：inbox 里没有完整行时，每读空一拍
    /// （[`EVENT_POLL_SLICE`]）就先问一句流还活着没有（[`Agent::stream_alive`]）——活着接着等，
    /// 到 `cap` 还没等到、或者流先断了，才炸，炸出来的话写清是哪一种。
    ///
    /// **高负载下曾偶发**：这条流上等的是真 socket 的字节，写死一个短的硬读超时会把"慢"报成
    /// "坏"（本机与 `cargo build` 并发跑时见过），所以"这一拍没数据"和"这次等超了"分开算。
    fn next_event(&mut self, cap: Duration) -> Value {
        let deadline = Instant::now() + cap;
        loop {
            if let Some(end) = find(&self.inbox, b"\n") {
                let line = String::from_utf8(self.inbox.drain(..end + 1).collect())
                    .expect("SSE 行是 UTF-8")
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                if let Some(data) = line.strip_prefix("data: ") {
                    println!("--- 事件\n{data}\n");
                    return serde_json::from_str(data).expect("事件体是 JSON");
                }
                if !line.is_empty() {
                    println!("--- 注释行\n{line}\n");
                }
                continue;
            }
            self.wait_for_input(deadline, cap, "一条 SSE 事件");
        }
    }

    /// 读一行原始 SSE（保活注释行走这里）。`cap` 的语义同 [`Agent::next_event`]。
    fn next_line(&mut self, cap: Duration) -> String {
        let deadline = Instant::now() + cap;
        loop {
            if let Some(end) = find(&self.inbox, b"\n") {
                let line = String::from_utf8(self.inbox.drain(..end + 1).collect())
                    .expect("SSE 行是 UTF-8")
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                if line.is_empty() {
                    continue; // SSE 里空行只是事件之间的分隔
                }
                println!("--- 原始行\n{line}\n");
                return line;
            }
            self.wait_for_input(deadline, cap, "长流上的下一行");
        }
    }

    /// 读一拍（最多 [`EVENT_POLL_SLICE`]）：新字节收进 `inbox`，读超时就是 [`Poll::Idle`]。
    fn poll(&mut self, slice: Duration) -> Poll {
        self.stream.set_read_timeout(Some(slice)).expect("设读超时");
        let mut chunk = [0u8; 4096];
        match self.stream.read(&mut chunk) {
            Ok(0) => Poll::Closed,
            Ok(read) => {
                self.inbox.extend_from_slice(&chunk[..read]);
                Poll::Bytes
            }
            Err(error) if is_read_timeout(&error) => Poll::Idle,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::NotConnected
                ) =>
            {
                Poll::Closed
            }
            Err(error) => panic!("读长流出错：{error}"),
        }
    }

    /// 流还活着吗（读超时之后用它把"对端慢"与"对端断了"分开）。
    ///
    /// `peek` 不消费字节：`Ok(0)` 是 FIN（对端关了），`Ok(n>0)` 说明有数据可读，超时错说明连接
    /// 还在、只是这一拍没有字节——这正是"高负载下别把慢当坏"要判的那一件事。
    fn stream_alive(&mut self) -> bool {
        let mut scratch = [0u8; 1];
        match self.stream.peek(&mut scratch) {
            Ok(0) => false,
            Ok(_) => true,
            Err(error) => is_read_timeout(&error),
        }
    }

    /// 等到 `inbox` 里有新字节（或流断了）：`deadline` 是这次等待的总上限（含之前读空的那几拍）。
    fn wait_for_input(&mut self, deadline: Instant, cap: Duration, what: &str) {
        loop {
            match self.poll(EVENT_POLL_SLICE) {
                Poll::Bytes => return,
                Poll::Closed => panic!("流在{what}到齐前关了"),
                Poll::Idle => assert!(
                    !deadline.saturating_duration_since(Instant::now()).is_zero(),
                    "等{what}超过 {cap:?}（{}）",
                    if self.stream_alive() {
                        "流还活着，只是没数据——多半是这一拍被调度压住了"
                    } else {
                        "流已经断了"
                    }
                ),
            }
        }
    }

    /// 只读到 `\r\n\r\n`（长流没有 `Content-Length`，体不在这里读）。
    fn read_head(&mut self) -> String {
        let end = loop {
            if let Some(end) = find(&self.inbox, b"\r\n\r\n") {
                break end;
            }
            let mut chunk = [0u8; 4096];
            let read = self.stream.read(&mut chunk).expect("读响应头");
            assert!(read > 0, "服务端在回完整响应头前关了连接");
            self.inbox.extend_from_slice(&chunk[..read]);
        };
        let head = String::from_utf8(self.inbox.drain(..end + 4).collect()).expect("头是 UTF-8");
        self.status = head
            .split(' ')
            .nth(1)
            .expect("状态码")
            .parse()
            .expect("状态码是数字");
        head
    }

    fn meta(&self) -> Value {
        json!({
            meta::key::PROTOCOL_VERSION: meta::PROTOCOL_VERSION,
            meta::key::CLIENT_INFO: { "name": "external-agent", "version": "0" },
            meta::key::CLIENT_CAPABILITIES: {},
        })
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 一次完整的读—改—写 + 开会话 + 关口，全走线。
#[test]
fn an_external_agent_drives_the_endpoints_over_loopback() {
    let Fixture { backend, .. } = fixture();
    let server = serve(options("agent"), Some(backend)).expect("起控制面");
    let mut agent = Agent::connect(server.addr(), server.token());

    // ① 列出端点。
    let listed = agent.call(1, "list_endpoints", json!({}));
    let structured = &listed["result"]["structuredContent"];
    assert_eq!(listed["result"]["resultType"], json!("complete"));
    assert!(listed["result"].get("isError").is_none(), "{listed}");
    assert_eq!(structured["device"]["tier"], json!("windows"));
    assert_eq!(
        structured["endpoints"].as_array().expect("endpoints").len(),
        2
    );

    // ② 看清单。
    let described = agent.call(2, "describe_endpoint", json!({ "endpoint": "speak" }));
    let structured = &described["result"]["structuredContent"];
    assert_eq!(structured["endpoint"], json!("speak"));
    assert_eq!(structured["manifest"]["schema_version"], json!(1));
    assert_eq!(structured["manifest"]["host"], json!("windows"));
    assert_eq!(
        structured["manifest"]["out"][0]["role"],
        json!("virtual_mic")
    );
    assert!(structured["editable"].as_array().expect("editable").len() >= 9);
    let manifest = structured["manifest"].clone();

    // ③ 原样发回：零差异（投影与反方向表自洽）。
    let round_trip = agent.call(
        3,
        "compose_endpoint",
        json!({ "endpoint": "speak", "composition": manifest, "apply": false }),
    );
    let structured = &round_trip["result"]["structuredContent"];
    assert_eq!(structured["changed"], json!([]), "{round_trip}");

    // ④ 改一格 → dry-run 拿 token → apply。
    let mut edited = manifest.clone();
    edited["session"]["params"]["target_language"] = json!("en");
    let dry_run = agent.call(
        4,
        "compose_endpoint",
        json!({ "endpoint": "speak", "composition": edited, "apply": false }),
    );
    let structured = &dry_run["result"]["structuredContent"];
    assert_eq!(
        structured["changed"][0]["path"],
        json!("session.params.target_language")
    );
    let token = structured["token"].as_str().expect("token").to_string();

    let applied = agent.call(
        5,
        "compose_endpoint",
        json!({ "endpoint": "speak", "composition": edited, "apply": true, "token": token }),
    );
    let structured = &applied["result"]["structuredContent"];
    assert_eq!(structured["applied"], json!(true), "{applied}");
    assert_eq!(
        structured["manifest"]["session"]["params"]["target_language"],
        json!("en")
    );

    // ⑤ 同一个 token 重放 → `isError` + `compose_token_stale`（两段式确认的全部意义）。
    let replayed = agent.call(
        6,
        "compose_endpoint",
        json!({ "endpoint": "speak", "composition": edited, "apply": true, "token": token }),
    );
    assert_eq!(replayed["result"]["isError"], json!(true), "{replayed}");
    assert_eq!(
        replayed["result"]["structuredContent"]["error"]["code"],
        json!("compose_token_stale")
    );

    // ⑥ 开会话 → 拿 handle 与字幕资源 URI。
    let opened = agent.call(7, "session_open", json!({ "endpoint": "speak" }));
    let structured = &opened["result"]["structuredContent"];
    let handle = structured["session"].as_str().expect("handle").to_string();
    assert_eq!(structured["state"], json!("ready"), "{opened}");
    assert_eq!(
        structured["transcript"],
        json!(format!("vox://session/{handle}/transcript"))
    );

    // ⑦ 关会话：幂等（第二次 stopped:false）。
    let closed = agent.call(8, "session_close", json!({ "session": handle }));
    assert_eq!(
        closed["result"]["structuredContent"]["stopped"],
        json!(true)
    );
    let closed = agent.call(9, "session_close", json!({ "session": handle }));
    assert_eq!(
        closed["result"]["structuredContent"]["stopped"],
        json!(false)
    );

    // ⑧ 结构性不合法走协议错误通道：`in: []` → `-32602` + `data.errors[0].kind == "missing_input"`。
    let mut broken = manifest.clone();
    broken["in"] = json!([]);
    let rejected = agent.call(
        10,
        "compose_endpoint",
        json!({ "endpoint": "speak", "composition": broken, "apply": false }),
    );
    assert_eq!(agent.status, 400, "结构性错误在 HTTP 上是 400");
    assert_eq!(rejected["error"]["code"], json!(-32602), "{rejected}");
    assert_eq!(
        rejected["error"]["data"]["errors"][0]["kind"],
        json!("missing_input")
    );

    server.shutdown();
}

/// 字幕资源面的全链（真 socket + 真账本 + SSE）：**订阅 → 发一条字幕 → 收到通知 → 读到内容**。
///
/// 这一条就是"外部 Agent 真能调通"的证据：客户端是裸 TCP 手写的 HTTP + SSE 读取，
/// 服务端是装配层用的同一个 `serve`，字幕是芯的字幕轨道真收了一条 delta 之后算出来的。
///
/// **抗负载**（这条**高负载下曾偶发**红）：① 每一拍等的都是真 socket 上的字节，写死一个短的硬
/// 读超时（曾用 5 s）在调度被压住时会红——现在等的是 [`event_wait`]（默认 30 s、
/// `VOX_MCP_TEST_EVENT_TIMEOUT_MS` 可覆写），且每读空一拍先 `peek` 判流是否还活着
/// （[`Agent::stream_alive`]）：流断了立刻红、流活着接着等。② 但光是放大上限**治不了**原始症状：
/// 高负载复跑时它曾等满 30 s 也没有那条通知——那是产品侧的窗口，订阅后的第一次变化被"拖到第一
/// 拍才对齐的水位"吞掉，已在 `mcp::subscriptions::accept`（基线在回 ack 之前取）修掉，
/// `tests/resources.rs::a_change_that_lands_right_after_subscribing_is_reported` 把它钉死。
///
/// 跑法（要看线上的原始字节）：
///
/// ```text
/// cargo test -p vox-mcp --test lifecycle -- --nocapture
/// ```
#[test]
fn an_external_agent_subscribes_to_the_transcript_over_sse() {
    let Fixture {
        runtime,
        control,
        backend,
    } = fixture();
    let server = serve(options("sse"), Some(backend)).expect("起控制面");

    // 两条连接：一条发请求，一条留着做长流（规范：每个请求各自一个 POST）。
    let mut agent = Agent::connect(server.addr(), server.token());
    let mut listener = Agent::connect(server.addr(), server.token());

    // ① 开会话拿 handle（资源 URI 里的 handle 是服务端签发的）。
    let opened = agent.call(1, "session_open", json!({ "endpoint": "speak" }));
    let handle = opened["result"]["structuredContent"]["session"]
        .as_str()
        .expect("handle")
        .to_string();
    let uri = format!("vox://session/{handle}/transcript");
    assert_eq!(
        opened["result"]["structuredContent"]["transcript"],
        json!(uri)
    );

    // ② `resources/list` 里能看见它，且带缓存提示。
    let listed = agent.request(2, "resources/list", json!({}), None);
    let result = &listed["result"];
    assert_eq!(result["resources"][0]["uri"], json!(uri));
    assert_eq!(
        result["resources"][0]["mimeType"],
        json!("application/json")
    );
    assert_eq!(result["ttlMs"], json!(0));
    assert_eq!(result["cacheScope"], json!("private"));

    // ③ 订阅（长流）：第一条必须是 ack，且只勾我们真会发的两类。
    let acknowledged = listener.listen(
        3,
        json!({
            "toolsListChanged": true,
            "resourcesListChanged": true,
            "resourceSubscriptions": [uri],
        }),
    );
    assert_eq!(
        acknowledged["method"],
        json!("notifications/subscriptions/acknowledged")
    );
    assert_eq!(
        acknowledged["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
        json!(3)
    );
    assert_eq!(
        acknowledged["params"]["notifications"],
        json!({ "resourcesListChanged": true, "resourceSubscriptions": [uri] })
    );

    // ④ 说一句话（走芯的公开面，和真实转写同一条路）→ 长流上收到 updated。
    speak(
        &runtime,
        &control,
        Pipeline::Speak,
        "Hello, nice",
        "Hello, nice",
        false,
    );
    let updated = listener.next_event(event_wait());
    assert_eq!(updated["method"], json!("notifications/resources/updated"));
    assert_eq!(updated["params"]["uri"], json!(uri));
    assert_eq!(
        updated["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
        json!(3)
    );

    // ⑤ 收到通知之后真能读到新字幕（快照，不是增量）。
    let read = agent.request(4, "resources/read", json!({ "uri": uri }), Some(&uri));
    assert_eq!(read["result"]["contents"][0]["uri"], json!(uri));
    assert_eq!(
        read["result"]["contents"][0]["mimeType"],
        json!("application/json")
    );
    assert_eq!(read["result"]["ttlMs"], json!(0));
    assert_eq!(read["result"]["cacheScope"], json!("private"));
    let snapshot: Value = serde_json::from_str(
        read["result"]["contents"][0]["text"]
            .as_str()
            .expect("快照 JSON"),
    )
    .expect("快照是合法 JSON");
    assert_eq!(snapshot["session"], json!(handle));
    assert_eq!(snapshot["endpoint"], json!("speak"));
    assert_eq!(snapshot["track"], json!("speak"));
    assert_eq!(snapshot["state"], json!("ready"));
    assert_eq!(snapshot["text"], json!("Hello, nice"));
    assert_eq!(snapshot["confirmed"], json!("Hello, nice"));
    assert_eq!(snapshot["last_delta_done"], json!(false));
    assert_eq!(snapshot["revision"], json!(1));
    assert_eq!(snapshot["notify_ms"], json!(250));

    // ⑥ 又说了半句 + 说完了 → 第二条 updated（去抖之后每个"真变了"一拍一条）。
    speak(
        &runtime,
        &control,
        Pipeline::Speak,
        " to meet you",
        "Hello, nice to meet you",
        true,
    );
    let updated = listener.next_event(event_wait());
    assert_eq!(updated["method"], json!("notifications/resources/updated"));
    assert_eq!(updated["params"]["uri"], json!(uri));
    let snapshot: Value = serde_json::from_str(
        agent.request(5, "resources/read", json!({ "uri": uri }), Some(&uri))["result"]["contents"]
            [0]["text"]
            .as_str()
            .expect("快照 JSON"),
    )
    .expect("快照是合法 JSON");
    assert_eq!(snapshot["text"], json!("Hello, nice to meet you"));
    assert_eq!(snapshot["last_delta_done"], json!(true));
    assert_eq!(snapshot["revision"], json!(2));

    // ⑦ 关会话：资源消失 → 长流上收到 list_changed；再读同一 URI → `-32602`。
    let closed = agent.call(6, "session_close", json!({ "session": handle }));
    assert_eq!(
        closed["result"]["structuredContent"]["stopped"],
        json!(true)
    );
    let changed = listener.next_event(event_wait());
    assert_eq!(
        changed["method"],
        json!("notifications/resources/list_changed")
    );
    assert_eq!(
        changed["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
        json!(3)
    );
    let gone = agent.request(7, "resources/read", json!({ "uri": uri }), Some(&uri));
    assert_eq!(agent.status, 400, "资源不存在在 HTTP 上是 400");
    assert_eq!(gone["error"]["code"], json!(-32602), "{gone}");
    assert_eq!(gone["error"]["data"]["uri"], json!(uri));

    // ⑧ 服务端收流：长流上先收到一条 result（规范 SHOULD：干净结束），之后连接才关。
    server.shutdown();
    let closing = listener.next_event(event_wait());
    assert_eq!(closing["id"], json!(3));
    assert_eq!(closing["result"]["resultType"], json!("complete"));
    assert_eq!(
        closing["result"]["_meta"][meta::key::SUBSCRIPTION_ID],
        json!(3)
    );
}

/// 长流空闲时的保活注释行（规范 SHOULD）+ 关连接划界（没有 `Content-Length`）。
///
/// 保活间隔在这里压到 300 ms（真值 15 s，等不起）；这条用例同时钉住"长流不带
/// `Content-Length`、靠关连接划界"——那是 HTTP/1.1 下唯一说得通的划界方式。
#[test]
fn a_quiet_stream_gets_keep_alive_comments_and_closes_without_content_length() {
    let Fixture { backend, .. } = fixture();
    let mut options = options("sse-keep-alive");
    options.sse_keep_alive_ms = 300;
    let server = serve(options, Some(backend)).expect("起控制面");

    let mut agent = Agent::connect(server.addr(), server.token());
    let mut listener = Agent::connect(server.addr(), server.token());

    let opened = agent.call(1, "session_open", json!({ "endpoint": "speak" }));
    let handle = opened["result"]["structuredContent"]["session"]
        .as_str()
        .expect("handle")
        .to_string();
    let uri = format!("vox://session/{handle}/transcript");

    // 只订阅"列表变化"：字幕一条都不勾，这条流会一直静着。
    let acknowledged = listener.listen(9, json!({ "resourcesListChanged": true }));
    assert_eq!(
        acknowledged["method"],
        json!("notifications/subscriptions/acknowledged")
    );

    // 静着的流上等一条保活注释行——`next_event` 会把它打印出来再继续等，所以这里直接读原始字节。
    let comment = listener.next_line(event_wait());
    assert_eq!(comment, ":", "规范：`:` 开头的行是保活注释，不携带数据");

    // 收流：先一条 result，再关连接（读返回 0）。
    server.shutdown();
    let closing = listener.next_event(event_wait());
    assert_eq!(closing["id"], json!(9));
    assert_eq!(closing["result"]["resultType"], json!("complete"));
    let mut scratch = [0u8; 64];
    listener
        .stream
        .set_read_timeout(Some(event_wait()))
        .expect("设读超时");
    assert_eq!(
        listener.stream.read(&mut scratch).expect("读流的尾巴"),
        0,
        "长流收尾后必须关连接（没有 Content-Length，靠它划界）"
    );
    let _ = uri;
}
/// **总闸**（`Settings.control.enabled`）关掉时，**已经开着的订阅流必须被收掉**——不是静默挂着。
///
/// 这一条是 S1 稿对总闸的承诺（`control.enabled` 是每道闸前面的那道）：用户在设置里把控制面
/// 整体关掉之后，客户端要拿到规范那条"干净结束"的 `result`（`resultType: "complete"` + 同一个
/// `subscriptionId`），而不是一条永远静着的连接；同时**新的**订阅流也不接受（否则客户端会陷入
/// "连上就被踢"的重连循环）。
///
/// 断言的三件事分别钉住三段代码：收流（`transport/http.rs::close_streams`）、
/// 读总闸（`watch_loop` 每拍问一次 `ControlBackend::control_enabled`）、
/// 拒新流（`mcp::listen`）。三条都在真 socket 上，客户端是裸 TCP。
#[test]
fn turning_the_master_switch_off_closes_the_open_subscriptions() {
    let Fixture {
        runtime, backend, ..
    } = fixture();
    let server = serve(options("gate-off"), Some(backend)).expect("起控制面");

    let mut agent = Agent::connect(server.addr(), server.token());
    let mut first = Agent::connect(server.addr(), server.token());
    let mut second = Agent::connect(server.addr(), server.token());

    // 一条会话 + 两条订阅流（两条各自一个 id，各自一条连接）。
    let opened = agent.call(1, "session_open", json!({ "endpoint": "speak" }));
    let handle = opened["result"]["structuredContent"]["session"]
        .as_str()
        .expect("handle")
        .to_string();
    let uri = format!("vox://session/{handle}/transcript");
    for (listener, id) in [(&mut first, 2), (&mut second, 3)] {
        let acknowledged = listener.listen(id, json!({ "resourceSubscriptions": [uri] }));
        assert_eq!(
            acknowledged["method"],
            json!("notifications/subscriptions/acknowledged")
        );
    }

    // 用户在设置里把总闸关掉（跟界面上拨的是同一格：`impl Grants for Runtime` 读的也是它）。
    Ledger::update_settings(&runtime, &mut |settings| settings.control.enabled = false);

    // 两条流都收到"干净结束"的 result，然后连接关掉——**不是**静默挂着。
    for (listener, id) in [(&mut first, 2), (&mut second, 3)] {
        let closing = listener.next_event(event_wait());
        assert_eq!(closing["id"], json!(id), "收尾的 result 要带原请求的 id");
        assert_eq!(closing["result"]["resultType"], json!("complete"));
        assert_eq!(
            closing["result"]["_meta"][meta::key::SUBSCRIPTION_ID],
            json!(id)
        );

        let mut scratch = [0u8; 64];
        listener
            .stream
            .set_read_timeout(Some(event_wait()))
            .expect("设读超时");
        assert_eq!(
            listener.stream.read(&mut scratch).expect("读流的尾巴"),
            0,
            "总闸关掉后这条流必须关连接（没有 Content-Length，靠它划界）"
        );
    }

    // 总闸关着时**新的**订阅流也不接受：一条普通错误（`-32603` + HTTP 500），不是一条长流。
    let refused = agent.request(
        4,
        "subscriptions/listen",
        json!({ "notifications": {} }),
        None,
    );
    assert_eq!(agent.status, 500, "推不了的流在 HTTP 上是 500，不是 200");
    assert_eq!(refused["error"]["code"], json!(-32603), "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("总开关")),
        "错误里要说清是总闸关着：{refused}"
    );

    // 对照：把总闸打开，同一个进程里订阅立刻又能开（收流不是"把服务弄坏了"）。
    // 换一条连接：长流收尾时服务端把那条连接关了（SSE 靠关连接划界），复用会撞上已关的连接。
    Ledger::update_settings(&runtime, &mut |settings| settings.control.enabled = true);
    let mut reopened_listener = Agent::connect(server.addr(), server.token());
    let reopened = reopened_listener.listen(5, json!({ "resourceSubscriptions": [uri] }));
    assert_eq!(
        reopened["method"],
        json!("notifications/subscriptions/acknowledged")
    );

    server.shutdown();
}
