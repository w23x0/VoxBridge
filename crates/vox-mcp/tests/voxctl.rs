//! `voxctl` 的**进程级**用例：5 个动作子命令 + stdio 桥，全部走真子进程 + 真 socket + 真账本。
//!
//! 断言的是外部可见的契约：退出码（`0` 成功 ｜ `1` 领域失败 ｜ `2` 传输/协议失败 ｜ `3` 用法错误）、
//! stdout 上到底是哪一串字节（`--json` = `structuredContent` 本身）、以及 stdio 桥上 stdout
//! **只有** MCP 消息。子命令的名字、用法与参数都来自 `actions::ACTIONS`——所以这里不重抄一遍
//! 工具名，而是拿表来比。
//!
//! 跑法（要看子进程的原始输出）：
//!
//! ```text
//! cargo test -p vox-mcp --test voxctl -- --nocapture
//! ```

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use vox_core::capability::HostFacts;
use vox_core::composition::HostKind;
use vox_core::event::{Pipeline, PipelineState};
use vox_core::ports::{Clock, PortResult};
use vox_core::runtime::{PipelineCommand, PipelineControl, Runtime};
use vox_core::settings::{ListenTarget, ModelProvider, Settings};
use vox_core::usage::Stamp;

use vox_mcp::actions::ACTIONS;
use vox_mcp::mcp::meta;
use vox_mcp::session::LedgerBackend;
use vox_mcp::transport::http::ServerOptions;
use vox_mcp::{serve, BoxedBackend};

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

/// 接到 `Start` 立刻报 `Ready` 的假控制面（真设备不在这条用例的射程里）。
struct ReadyControl {
    runtime: Runtime,
    started: Mutex<Vec<(Pipeline, u64)>>,
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

/// 一套跑得起来的本机：真账本 + 真后端（`LedgerBackend`）+ 能喂字幕的假控制面。
struct Fixture {
    runtime: Runtime,
    backend: BoxedBackend,
}

/// `edit` 在默认设置上再改几格（用例要的总闸 / 授权位都从这里拨）。
fn fixture(edit: impl FnOnce(&mut Settings)) -> Fixture {
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
    edit(&mut settings);

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
    runtime.set_control(control);
    let backend: BoxedBackend = Box::new(LedgerBackend::new(runtime.clone(), runtime.clone()));
    Fixture { runtime, backend }
}

/// 每个用例一个独立目录（用例并行跑，共用一个握手文件会互相踩）。
///
/// **纯函数**：只算路径，不碰文件系统——起完服务之后再算一次不该把刚写出的握手文件擦掉
/// （清目录是 [`options`] 的事，它只在起服务之前跑一次）。
fn state_file(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!("vox-mcp-voxctl-{}-{name}", std::process::id()))
        .join("control.json")
}

fn options(name: &str) -> ServerOptions {
    let path = state_file(name);
    let _ = std::fs::remove_dir_all(path.parent().expect("父目录"));
    ServerOptions::new(path)
}

/// 跑一次 `voxctl`（真子进程）：`(退出码, stdout, stderr)`。
fn voxctl(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_voxctl"))
        .args(args)
        .output()
        .expect("跑 voxctl");
    println!(
        "--- voxctl {}\n退出码 {}\n--- stdout\n{}\n--- stderr\n{}",
        args.join(" "),
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// 一条 `tools/call` 的响应（进程内，用来跟 CLI 的 stdout 逐字节比）。
fn in_process_call(runtime: &Runtime, id: i64, name: &str, arguments: Value) -> Value {
    let mut backend = LedgerBackend::new(runtime.clone(), runtime.clone());
    let message = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {
            "_meta": {
                meta::key::PROTOCOL_VERSION: meta::PROTOCOL_VERSION,
                meta::key::CLIENT_INFO: { "name": "in-process", "version": "0" },
                meta::key::CLIENT_CAPABILITIES: {},
            },
            "name": name,
            "arguments": arguments,
        },
    });
    vox_mcp::handle(&message, Some(&mut backend))
        .response()
        .expect("tools/call 必须有响应")
}

// --- 命令面（由动作清单生成）------------------------------------------------------

#[test]
fn the_help_lists_every_action_subcommand_and_the_transport_entries() {
    let (code, stdout, _) = voxctl(&["--help"]);
    assert_eq!(code, 0);
    for action in ACTIONS {
        assert!(
            stdout.contains(&action.cli_command()),
            "--help 必须列出 {}：{stdout}",
            action.cli_command()
        );
    }
    assert!(stdout.contains("serve-stdio"), "{stdout}");
    assert!(stdout.contains("--probe"), "{stdout}");
    // 退出码表是契约的一部分，必须写在帮助里。
    assert!(stdout.contains("3 用法错误"), "{stdout}");
}

#[test]
fn a_subcommand_usage_is_generated_from_its_input_schema() {
    // 参数名、类型、枚举取值、说明、必填——全部来自 `compose_endpoint` 的 `inputSchema`。
    let (code, stdout, _) = voxctl(&["compose-endpoint", "--help"]);
    assert_eq!(code, 0);
    for flag in [
        "--endpoint",
        "--composition",
        "--apply",
        "--no-apply",
        "--token",
    ] {
        assert!(stdout.contains(flag), "用法里必须有 {flag}：{stdout}");
    }
    assert!(
        stdout.contains("speak / listen"),
        "枚举取值要列出来：{stdout}"
    );
    assert!(stdout.contains("（必填）"), "必填要标出来：{stdout}");
    assert!(
        stdout.contains("完整清单"),
        "说明来自 schema 的 description：{stdout}"
    );

    // 无参数的工具也有用法（不写"参数"表，但不许报错）。
    let (code, stdout, _) = voxctl(&["list-endpoints", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("（没有参数）"), "{stdout}");

    // 每个动作子命令的用法都能出来（表驱动，不重抄名字）。
    for action in ACTIONS {
        let (code, stdout, _) = voxctl(&[&action.cli_command(), "--help"]);
        assert_eq!(code, 0, "{} --help", action.cli_command());
        assert!(
            stdout.contains(&format!(
                "voxctl {} —— {}",
                action.cli_command(),
                action.title
            )),
            "用法头必须来自动作表：{stdout}"
        );
    }
}

#[test]
fn usage_errors_are_exit_code_three() {
    // 缺 `--state-file`：CLI 不猜目录，缺了就是用法错误。
    let (code, _, stderr) = voxctl(&["list-endpoints"]);
    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("--state-file"), "{stderr}");

    // 缺必填参数。
    let (code, _, stderr) = voxctl(&["describe-endpoint", "--state-file", "/tmp/x.json"]);
    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("--endpoint"), "{stderr}");

    // 开关后面没跟值。
    let (code, _, stderr) = voxctl(&["describe-endpoint", "--endpoint"]);
    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("要跟值"), "{stderr}");

    // 不认识的开关 / 不认识的子命令。
    let (code, _, stderr) = voxctl(&[
        "describe-endpoint",
        "--endpoint",
        "speak",
        "--bogus",
        "x",
        "--state-file",
        "/tmp/x.json",
    ]);
    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("--bogus"), "{stderr}");
    let (code, _, stderr) = voxctl(&["bogus-command"]);
    assert_eq!(code, 3, "{stderr}");
    assert!(stderr.contains("不认识的子命令"), "{stderr}");

    // `serve-stdio` 也一样：缺 `--state-file` → 3。
    let (code, _, stderr) = voxctl(&["serve-stdio"]);
    assert_eq!(code, 3, "{stderr}");
}

// --- 动作子命令打真控制面 ---------------------------------------------------------

#[test]
fn list_endpoints_over_the_cli_is_byte_identical_to_the_structured_content() {
    // 验收 §4-18 的机械证明：CLI 的 stdout（`--json`）与 MCP 面的 `structuredContent`
    // 是**同一串字节**——CLI 只是把它从 HTTP 上取回来打出去。
    let Fixture { runtime, backend } = fixture(|_| {});
    let server = serve(options("cli-bytes"), Some(backend)).expect("起控制面");
    let path = state_file("cli-bytes").to_string_lossy().into_owned();

    let (code, stdout, stderr) = voxctl(&["list-endpoints", "--state-file", &path, "--json"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stderr, "", "成功路径不许往 stderr 写东西");

    let expected = in_process_call(&runtime, 1, "list_endpoints", json!({}))["result"]
        ["structuredContent"]
        .to_string();
    assert_eq!(
        stdout.trim_end(),
        expected,
        "CLI 的 --json 必须逐字节等于 structuredContent"
    );

    // 缺省（不带 `--json`）是一行中文摘要，且 stdout 只有这一行。
    let (code, stdout, _) = voxctl(&["list-endpoints", "--state-file", &path]);
    assert_eq!(code, 0);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    assert!(stdout.contains("2 个端点"), "{stdout}");
    assert!(stdout.contains("档位 windows"), "{stdout}");

    server.shutdown();
}

#[test]
fn a_bad_enum_value_goes_through_the_protocol_layer_and_exits_two() {
    // 验收 §4-20：`--endpoint nope` 不是用法错误（退出码 3），而是协议层的 `-32602`（退出码 2）
    // ——值的对错只有一个判定者，就是服务端。
    let Fixture { backend, .. } = fixture(|_| {});
    let server = serve(options("cli-enum"), Some(backend)).expect("起控制面");
    let path = state_file("cli-enum").to_string_lossy().into_owned();

    let (code, stdout, stderr) = voxctl(&[
        "describe-endpoint",
        "--endpoint",
        "nope",
        "--state-file",
        &path,
    ]);
    assert_eq!(code, 2, "{stderr}");
    assert_eq!(stdout, "", "协议错误不许往 stdout 写东西");
    assert!(stderr.contains("-32602"), "{stderr}");
    assert!(stderr.contains("describe_endpoint"), "{stderr}");

    server.shutdown();
}

#[test]
fn a_domain_failure_is_exit_code_one() {
    // 世界不允许（没授权）：`isError` 工具结果 → 退出码 1，不是 2。
    let Fixture { backend, .. } = fixture(|settings| settings.control.allow_microphone = false);
    let server = serve(options("cli-domain"), Some(backend)).expect("起控制面");
    let path = state_file("cli-domain").to_string_lossy().into_owned();

    let (code, stdout, stderr) =
        voxctl(&["session-open", "--endpoint", "speak", "--state-file", &path]);
    assert_eq!(code, 1, "{stderr}");
    assert_eq!(stdout, "", "缺省输出：领域失败只写 stderr");
    assert!(stderr.contains("permission_denied"), "{stderr}");
    assert!(
        stderr.contains("允许麦克风"),
        "hint 要指到具体开关：{stderr}"
    );

    // `--json` 时 stdout 就是 `structuredContent`（领域失败是工具结果，不是协议错误）。
    let (code, stdout, _) = voxctl(&[
        "session-open",
        "--endpoint",
        "speak",
        "--state-file",
        &path,
        "--json",
    ]);
    assert_eq!(code, 1);
    let structured: Value =
        serde_json::from_str(stdout.trim_end()).expect("--json 必须是合法 JSON");
    assert_eq!(structured["error"]["code"], json!("permission_denied"));
    assert_eq!(
        structured["error"]["detail"]["permission"],
        json!("microphone")
    );

    server.shutdown();
}

#[test]
fn a_control_plane_that_is_not_there_is_exit_code_two() {
    // ① 握手文件不在（控制面没起 / 路径不对）。
    let (code, _, stderr) = voxctl(&[
        "list-endpoints",
        "--state-file",
        "/tmp/voxctl-definitely-not-here/control.json",
    ]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("读不到握手文件"), "{stderr}");

    // ② 文件在，但那个端口上没人（凭据指向一个死掉的控制面）。
    let path = state_file("cli-dead");
    std::fs::create_dir_all(path.parent().expect("父目录")).expect("建目录");
    std::fs::write(&path, r#"{"port":1,"token":"x","pid":1}"#).expect("写握手文件");
    let (code, _, stderr) = voxctl(&["list-endpoints", "--state-file", &path.to_string_lossy()]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("连不上本机控制面"), "{stderr}");

    // ③ 文件在、端口也有人，但**凭据不对**：控制面回 401（规范要求空 body），CLI 如实报
    //    "看不懂的响应"（退出码 2），而不是把 401 编成一条 JSON-RPC 错误。
    let Fixture { backend, .. } = fixture(|_| {});
    let server = serve(options("cli-wrong-token"), Some(backend)).expect("起控制面");
    let path = state_file("cli-wrong-token");
    std::fs::write(
        &path,
        format!(
            r#"{{"port":{},"token":"wrong","pid":1}}"#,
            server.addr().port()
        ),
    )
    .expect("写错凭据的握手文件");
    let (code, _, stderr) = voxctl(&["list-endpoints", "--state-file", &path.to_string_lossy()]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("HTTP 401"), "{stderr}");
    server.shutdown();

    // ④ 文件在，但里面不是握手文件（不是 JSON / 缺 token）。
    let path = state_file("cli-not-a-handshake");
    std::fs::create_dir_all(path.parent().expect("父目录")).expect("建目录");
    std::fs::write(&path, "这不是 JSON").expect("写坏文件");
    let (code, _, stderr) = voxctl(&["list-endpoints", "--state-file", &path.to_string_lossy()]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("不是合法 JSON"), "{stderr}");
    std::fs::write(&path, r#"{"port":1234}"#).expect("写缺 token 的文件");
    let (code, _, stderr) = voxctl(&["list-endpoints", "--state-file", &path.to_string_lossy()]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("不是握手文件"), "{stderr}");
}

// --- stdio 桥 -------------------------------------------------------------------

/// 一条 stdio 桥（真子进程）：逐行写 stdin、逐行读 stdout，stderr 收到一个共享缓冲里。
///
/// 读 stdout 走**后台线程 + 通道**（不是直接阻塞读）：管道上设不了超时，直接读会把"桥卡住了"
/// 变成"用例永远挂着"，而挂着是最难查的失败。超时到点就 panic，并把手里的 stderr 一起打出来。
struct Bridge {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
}

impl Bridge {
    fn start(state_file: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_voxctl"))
            .arg("serve-stdio")
            .arg("--state-file")
            .arg(state_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("起 stdio 桥");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });

        let buffer = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&buffer);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { return };
                let mut buffer = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                buffer.push_str(&line);
                buffer.push('\n');
            }
        });

        Self {
            child,
            stdin,
            lines,
            stderr: buffer,
        }
    }

    /// 一条消息（一行），写完就 flush。
    fn send(&mut self, message: &Value) {
        println!("--- 桥 stdin\n{message}");
        writeln!(self.stdin, "{message}").expect("写桥的 stdin");
        self.stdin.flush().expect("flush");
    }

    /// stdout 上的下一行（MCP 消息）。超时 = 桥没回话。
    fn next_line(&self, timeout: Duration) -> String {
        match self.lines.recv_timeout(timeout) {
            Ok(line) => {
                println!("--- 桥 stdout\n{line}");
                line
            }
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "stdio 桥 {timeout:?} 内没写出下一行；stderr：\n{}",
                    self.stderr()
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("stdio 桥的 stdout 关了；stderr：\n{}", self.stderr())
            }
        }
    }

    /// stdout 上**不该**有东西：`timeout` 内没有新行才算过。
    fn assert_silent(&self, timeout: Duration) {
        match self.lines.recv_timeout(timeout) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(line) => panic!("stdout 上多了一行（规范：通知不回任何东西）：{line}"),
            Err(RecvTimeoutError::Disconnected) => panic!("stdio 桥的 stdout 关了"),
        }
    }

    fn stderr(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 等 stderr 上出现某段话（桥的日志是异步写的）。
    fn wait_for_stderr(&self, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let text = self.stderr();
            if text.contains(needle) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "stderr 里没等到 {needle}：\n{text}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// 关 stdin（宿主收摊）→ 等它自己退出，返回退出码。
    fn finish(self) -> i32 {
        drop(self.stdin);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut child = self.child;
        loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                println!("--- 桥退出：{:?}", status.code());
                return status.code().unwrap_or(-1);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                panic!("stdio 桥没在 10 s 内收工");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn meta_object() -> Value {
    json!({
        meta::key::PROTOCOL_VERSION: meta::PROTOCOL_VERSION,
        meta::key::CLIENT_INFO: { "name": "host", "version": "0" },
        meta::key::CLIENT_CAPABILITIES: {},
    })
}

#[test]
fn the_stdio_bridge_forwards_a_tools_list_and_stays_in_sync() {
    let Fixture { backend, .. } = fixture(|_| {});
    let server = serve(options("bridge"), Some(backend)).expect("起控制面");
    let path = state_file("bridge");

    let mut bridge = Bridge::start(&path);

    // ① 一条请求 → 一行响应（stdout 上恰好一行合法 JSON-RPC）。
    bridge.send(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/list",
        "params": { "_meta": meta_object() },
    }));
    let line = bridge.next_line(Duration::from_secs(10));
    let response: Value = serde_json::from_str(&line).expect("桥的 stdout 必须是合法 JSON");
    assert_eq!(response["jsonrpc"], json!("2.0"));
    assert_eq!(response["id"], json!(7));
    assert_eq!(response["result"]["resultType"], json!("complete"));
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        ACTIONS.iter().map(|action| action.name).collect::<Vec<_>>(),
        "工具清单与顺序都由动作表说了算"
    );

    // ② 通知 → 规范要求 202 无 body → stdout 上一个字都不许多。
    bridge.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/whatever",
        "params": { "_meta": meta_object() },
    }));
    bridge.assert_silent(Duration::from_millis(300));

    // ③ 再发一条请求：证明桥还在（通知没把它的行同步打乱）。
    bridge.send(&json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": { "_meta": meta_object(), "name": "list_endpoints", "arguments": {} },
    }));
    let line = bridge.next_line(Duration::from_secs(10));
    let response: Value = serde_json::from_str(&line).expect("合法 JSON");
    assert_eq!(response["id"], json!(8));
    assert_eq!(
        response["result"]["structuredContent"]["endpoints"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    // ④ 一条上游会拒的请求也照样原样回来（桥不替服务端解释协议）。
    bridge.send(&json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": { "_meta": meta_object(), "name": "nope", "arguments": {} },
    }));
    let line = bridge.next_line(Duration::from_secs(10));
    let response: Value = serde_json::from_str(&line).expect("合法 JSON");
    assert_eq!(response["id"], json!(9));
    assert_eq!(response["error"]["code"], json!(-32602));

    // ⑤ stdin 关了 = 宿主收摊：桥干净退出。
    assert_eq!(bridge.finish(), 0);
    server.shutdown();
}

#[test]
fn the_stdio_bridge_forwards_the_sse_stream_and_maps_cancellation() {
    let Fixture { backend, .. } = fixture(|_| {});
    let server = serve(options("bridge-sse"), Some(backend)).expect("起控制面");
    let path = state_file("bridge-sse");

    // 先开一条会话（走 CLI 的动作子命令，顺手再证一次它能跑）。
    let (code, stdout, stderr) = voxctl(&[
        "session-open",
        "--endpoint",
        "speak",
        "--state-file",
        &path.to_string_lossy(),
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let opened: Value = serde_json::from_str(stdout.trim_end()).expect("structuredContent");
    let handle = opened["session"].as_str().expect("handle").to_string();
    let uri = opened["transcript"]
        .as_str()
        .expect("transcript")
        .to_string();

    let mut bridge = Bridge::start(&path);

    // ① `subscriptions/listen` 的"回答"是一条长流：桥把 SSE 的每条 `data:` 写成一行。
    bridge.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "subscriptions/listen",
        "params": {
            "_meta": meta_object(),
            "notifications": { "resourcesListChanged": true, "resourceSubscriptions": [uri] },
        },
    }));
    let line = bridge.next_line(Duration::from_secs(10));
    let acknowledged: Value = serde_json::from_str(&line).expect("合法 JSON");
    assert_eq!(
        acknowledged["method"],
        json!("notifications/subscriptions/acknowledged")
    );
    assert_eq!(
        acknowledged["params"]["_meta"][meta::key::SUBSCRIPTION_ID],
        json!(2)
    );

    // ② 长流开着的时候，别的请求照样能过（一条消息一个线程——单线程会全堵在流后面）。
    bridge.send(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "resources/read",
        "params": { "_meta": meta_object(), "uri": format!("vox://session/{handle}/transcript") },
    }));
    let line = bridge.next_line(Duration::from_secs(10));
    let read: Value = serde_json::from_str(&line).expect("合法 JSON");
    assert_eq!(read["id"], json!(3));
    assert_eq!(read["result"]["contents"][0]["uri"], json!(uri));

    // ③ stdio 的取消（`notifications/cancelled`）→ HTTP 的取消（关掉上游那条 POST 的响应流）。
    //    规范在 HTTP 上没有这条通知，桥负责把前者映射成后者。
    bridge.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": { "requestId": 2, "reason": "宿主不要了" },
    }));
    let stderr = bridge.wait_for_stderr("已收掉上游流 #2", Duration::from_secs(5));
    assert!(stderr.contains("notifications/cancelled"), "{stderr}");

    // ④ 取消之后桥照常干活（没崩、没卡）。
    bridge.send(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/list",
        "params": { "_meta": meta_object() },
    }));
    let line = bridge.next_line(Duration::from_secs(10));
    let listed: Value = serde_json::from_str(&line).expect("合法 JSON");
    assert_eq!(listed["id"], json!(4));
    assert_eq!(
        listed["result"]["tools"].as_array().map(Vec::len),
        Some(ACTIONS.len())
    );

    assert_eq!(bridge.finish(), 0);

    // ⑤ 会话与上游流都收干净了：服务端正常停服（不等超时）。
    server.shutdown();
}

#[test]
fn the_stdio_bridge_refuses_to_start_without_a_handshake_file() {
    let (code, stdout, stderr) = voxctl(&[
        "serve-stdio",
        "--state-file",
        "/tmp/voxctl-definitely-not-here/control.json",
    ]);
    assert_eq!(code, 2, "{stderr}");
    assert_eq!(stdout, "", "桥的 stdout 只能有 MCP 消息");
    assert!(stderr.contains("读不到握手文件"), "{stderr}");
}

/// 验收 §4-21 的用法：`printf '<一条 JSON-RPC>\n' | voxctl serve-stdio`——**stdin 紧接着就是
/// EOF**，而 stdout 上必须恰好一行合法 JSON-RPC 响应。
///
/// 这一条钉住的是桥收尾时的顺序：stdin 一关就退出的话，在飞的转发线程会被连人带响应一起掐掉
/// （stdout 上一个字都没有）。所以桥在退出前要等已经在飞的那几条写完。
#[test]
fn a_one_shot_pipe_gets_its_response_before_the_bridge_exits() {
    let Fixture { backend, .. } = fixture(|_| {});
    let server = serve(options("bridge-one-shot"), Some(backend)).expect("起控制面");
    let path = state_file("bridge-one-shot");

    let mut child = Command::new(env!("CARGO_BIN_EXE_voxctl"))
        .arg("serve-stdio")
        .arg("--state-file")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起 stdio 桥");

    // 写一条就关 stdin（`take()` 之后 drop = EOF）。
    {
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(
            stdin,
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list",
                "params": { "_meta": meta_object() },
            })
        )
        .expect("写一条请求");
    }

    let output = child.wait_with_output().expect("等桥退出");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    println!(
        "--- 一次性管道\n退出码 {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );

    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout 上必须恰好一行合法 JSON-RPC：{stdout}"
    );
    let response: Value = serde_json::from_str(stdout.trim_end()).expect("合法 JSON-RPC");
    assert_eq!(response["id"], json!(1));
    assert_eq!(
        response["result"]["tools"].as_array().map(Vec::len),
        Some(ACTIONS.len())
    );

    server.shutdown();
}

/// 上游的传输失败（连不上 / 凭据不对 / 看不懂的响应）必须变成**一条** `-32603`（带原请求的 id）：
/// 宿主在等一条响应，而"等一个永远不会来的响应"比一条说得清的错误坏得多。
#[test]
fn the_bridge_reports_an_upstream_failure_as_a_json_rpc_error() {
    let Fixture { backend, .. } = fixture(|_| {});
    let server = serve(options("bridge-bad-token"), Some(backend)).expect("起控制面");
    let path = state_file("bridge-bad-token");
    std::fs::write(
        &path,
        format!(
            r#"{{"port":{},"token":"wrong","pid":1}}"#,
            server.addr().port()
        ),
    )
    .expect("写错凭据的握手文件");

    let mut child = Command::new(env!("CARGO_BIN_EXE_voxctl"))
        .arg("serve-stdio")
        .arg("--state-file")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("起 stdio 桥");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        writeln!(
            stdin,
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": "req-1",
                "method": "tools/list",
                "params": { "_meta": meta_object() },
            })
        )
        .expect("写一条请求");
    }

    let output = child.wait_with_output().expect("等桥退出");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    println!(
        "--- 凭据不对的桥\n退出码 {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );

    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let response: Value = serde_json::from_str(stdout.trim_end()).expect("合法 JSON-RPC");
    assert_eq!(response["id"], json!("req-1"), "id 要原样带回去");
    assert_eq!(response["error"]["code"], json!(-32603));
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("HTTP 401")),
        "错误里要说清上游是怎么失败的：{response}"
    );

    server.shutdown();
}
