//! 本机 Streamable HTTP 传输（MCP 2026-07-28）。
//!
//! 形状（设计稿 §2.2.1 / §2.5.1）：只绑 `127.0.0.1`、单路径 `/mcp`、**POST-only**、
//! 无 session、无 GET、无 SSE 续传。安全面三件：`Origin` 一律 403、`Authorization: Bearer
//! <token>` 常数时间比较、握手文件 `0600`。
//!
//! ```text
//!   accept ──▶ 解析请求行 ─┬─ 路径不是 /mcp        → 404
//!               与请求头  ├─ 方法不是 POST        → 405 + Allow: POST
//!                         ├─ 带 Origin            → 403（无 body）
//!                         ├─ token 缺/错          → 401（无 body）
//!                         ├─ 请求头超过 16 KiB    → 431
//!                         ├─ 体的分帧不合法       → 400 / 411 / 413 / 501
//!                         └─ 请求头与 body 不符    → 400 + -32020
//!                                    │
//!                                    ▼
//!                          crate::handle（与 CLI 同一个函数）
//!                                    │
//!            Answer ─┬─ Response ─▶ 200 JSON ｜ Silence ─▶ 202 无 body
//!                    └─ Stream   ─▶ 200 text/event-stream（一条长流，见下）
//! ```
//!
//! **长流**（`subscriptions/listen`，规范 Major 4）：协议层把这条请求的"回答"表示成
//! [`Answer::Stream`]，传输面只做四件事——写 SSE 头、把 ack 写出去、每 tick 把协议层算好的
//! 通知写出去、断开时收摊。**协议语义一行都不在这里**：要发什么、按什么过滤、水位到哪，
//! 全是 `mcp::subscriptions` 的事。
//!
//! ```text
//!   ticker 线程（全服务一条）                   每条流一个连接线程
//!   ┌───────────────────────────┐            ┌──────────────────────────┐
//!   │ 有订阅流吗？没有就睡       │            │ 写头 → 写 ack → 循环     │
//!   │ 有 → backend.poll_resources│  mpsc      │  recv_timeout(250ms)：    │
//!   │     → 按每条流的过滤投递 ──┼───────────▶│   收到 → 写 data: …      │
//!   └───────────────────────────┘            │   超时 → 看 stop/对端/保活│
//!                                            └──────────────────────────┘
//! ```
//!
//! 三条规范细节都照做了：起流带 `X-Accel-Buffering: no`；空闲每
//! [`ServerOptions::sse_keep_alive_ms`] 发一个 `:` 注释行保活；服务端主动收流前**先回一条
//! result**（`resultType: "complete"` + 同一个 `subscriptionId`），客户端据此区分"干净结束"
//! 与"意外断开"。没有 `Content-Length` 的长流靠**关连接**划界，所以那条响应带
//! `Connection: close`（HTTP/1.1 下这是必须的，不是选择）。
//!
//! 规范出处（2026-07-28）：`basic/transports/streamable-http` 的 `#request-metadata` /
//! `#server-validation` / `#security--endpoint` / `#sending-messages` / `#receiving-messages` /
//! `#earlier-streamable-http-revisions`；`basic/patterns/subscriptions`。
//!
//! 三条刻意的取舍：
//!
//! - **体上限、头上限、空闲超时、并发上限**全部照 §2.5.1 的定值（16 KiB / 1 MiB / 60 s / 8）。
//!   超限的响应码规范没规定，这里取 HTTP 里语义最近的那几个（431 / 413 / 411 / 501）。
//! - **拒绝请求时先排空对端还没送完的字节再关连接**（[`Conn::reject`]）：带着未读数据
//!   `close()` 会让内核回 RST，把刚写出的 401/413 冲掉——客户端看到的是 "connection reset"，
//!   而不是我们真实的拒绝理由。
//! - **一条长流占一个连接名额**（并发上限 8）：8 条订阅流同时在跑就是满了。本机控制面的真实
//!   用法是"一个宿主 + 一个 CLI"，这条上限够用；写在这里是为了别让人以为是漏算的。

use std::borrow::Cow;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};

use crate::handlers::BoxedBackend;
use crate::jsonrpc::{self, code, ErrorObject};
use crate::mcp::subscriptions::Subscription;
use crate::mcp::{self, meta, Answer};
use crate::resources::ResourceTick;

/// 唯一路径。单路径是设计选择，不是省事：多一条路径就多一套路由与校验面。
pub const PATH: &str = "/mcp";

/// token 熵（设计稿 §2.5.1）：`getrandom` 取 32 字节 → base64url 无填充 43 字符。
const TOKEN_BYTES: usize = 32;

/// 连接线程里 `read` 的轮询节拍：每 250 ms 醒一次看 stop 标志与空闲时限。
///
/// 它有三个作用：空闲 60 s 是**在这个节拍上算的**（不是靠 socket 的读超时）；
/// [`ServerHandle::shutdown`] 能在一个节拍内把所有连接线程收干净——否则关机要等满 60 s；
/// SSE 长流也在这个节拍上看一眼"停服没有、对端还在不在、该不该发保活注释行"。
const POLL_MS: u64 = 250;

/// 没有订阅流时 ticker 睡多久（不碰账本，只等被叫醒或到点再看一眼）。
const IDLE_WATCH_MS: u64 = 1_000;

/// 长流空闲多久发一个 SSE 注释行保活（设计稿 §2.3.1 第 14 条：15 s）。
const SSE_KEEP_ALIVE_MS: u32 = 15_000;

/// 服务端参数。
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// 监听地址。**只允许回环**（`127.0.0.1` / `::1`），别的地址 [`serve`] 直接拒绝。
    pub bind: SocketAddr,
    /// 握手文件（装配层给 `<app_config_dir>/control.json`）。端口、token、pid 写在这里，
    /// CLI 与宿主读它拿地址与凭据。
    pub state_file: PathBuf,
    /// 请求头上限（默认 16 KiB）。
    pub max_header_bytes: usize,
    /// 请求体上限（默认 1 MiB）。
    pub max_body_bytes: usize,
    /// 空闲多久关连接（默认 60 s）。
    pub idle_timeout_ms: u32,
    /// 并发连接上限（默认 8）。满了就让接受循环等，**不**丢连接、也不自造规范里没有的状态码。
    pub max_connections: usize,
    /// SSE 长流空闲多久发一个保活注释行（默认 15 s）。
    pub sse_keep_alive_ms: u32,
}

impl ServerOptions {
    /// 默认值全部照设计稿 §2.5.1：`127.0.0.1:0`（端口由系统分配）、16 KiB / 1 MiB / 60 s / 8；
    /// 保活 15 s 照 §2.3.1 第 14 条。
    pub fn new(state_file: impl Into<PathBuf>) -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            state_file: state_file.into(),
            max_header_bytes: 16 << 10,
            max_body_bytes: 1 << 20,
            idle_timeout_ms: 60_000,
            max_connections: 8,
            sse_keep_alive_ms: SSE_KEEP_ALIVE_MS,
        }
    }

    pub fn bind(mut self, bind: SocketAddr) -> Self {
        self.bind = bind;
        self
    }
}

/// 一个跑起来的控制面。`shutdown`（或 `drop`）会停监听、断连接、并擦掉自己写的握手文件。
pub struct ServerHandle {
    addr: SocketAddr,
    token: String,
    state_file: PathBuf,
    stop: Arc<AtomicBool>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    accept: Option<JoinHandle<()>>,
    /// 资源变更的 ticker（全服务一条）。
    watch: Option<JoinHandle<()>>,
    /// 叫醒 ticker 用（停服、新订阅流）。
    wake: Arc<Wake>,
}

impl ServerHandle {
    /// 实际监听的地址（`ServerOptions::bind` 的端口是 0 时，端口只有这里才知道）。
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 本次进程的 token（43 字符 base64url）。CLI 与宿主靠它过 [`ServerOptions::state_file`]
    /// 取，这里也给一份，省得为了拿 token 去读文件。
    pub fn token(&self) -> &str {
        &self.token
    }

    /// 停服：置停止标志 → 唤醒接受线程 → join 连接线程 → 擦掉握手文件。
    ///
    /// 正在执行的一次 `tools/call` **不打断**（后端是同步阻塞的，半截的配置写入比慢一点更糟）；
    /// 所以最坏情况下的关机时间 = 当前那一次调用跑完的时间。
    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        let Some(accept) = self.accept.take() else {
            return; // 已经停过
        };
        self.stop.store(true, Ordering::SeqCst);
        // ticker 睡在条件变量上：叫醒它，它睁眼就看到 stop（否则最长要等一个 notify 周期）。
        self.wake.wake();
        // 接受线程卡在 accept() 上，叫醒它：向自己连一条连接，它睁眼就看到 stop。
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        let _ = accept.join();
        // ticker 先收（它不该再往正在收摊的流里投递），再收连接线程。
        if let Some(watch) = self.watch.take() {
            let _ = watch.join();
        }
        let workers = std::mem::take(&mut *lock(&self.workers));
        for worker in workers {
            let _ = worker.join();
        }
        remove_handshake_if_ours(&self.state_file, &self.token);
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

/// 起一个控制面 HTTP 服务。
///
/// `backend` 是唯一能碰账本/设备的入口，由外壳注入（`voxctl serve` 传 `None`：它是纯协议面
/// 的调试入口，那时 `tools/call` 会如实地回 `-32603`，而不是假成功）。
pub fn serve(options: ServerOptions, backend: Option<BoxedBackend>) -> io::Result<ServerHandle> {
    if !options.bind.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "控制面只允许绑回环地址（设计稿 §2.5.1；不做 0.0.0.0 / 远程），收到 {}",
                options.bind
            ),
        ));
    }

    let listener = TcpListener::bind(options.bind)?;
    let addr = listener.local_addr()?;
    let token = random_token()?;
    write_handshake(&options.state_file, addr.port(), &token)?;

    let state_file = options.state_file.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let workers = Arc::new(Mutex::new(Vec::new()));
    let wake = Arc::new(Wake::default());
    let context = Arc::new(Context {
        slots: Slots::new(options.max_connections),
        options,
        token: token.clone(),
        backend: Mutex::new(backend),
        streams: Mutex::new(Vec::new()),
        next_stream: AtomicU64::new(1),
        wake: Arc::clone(&wake),
    });

    let accept = {
        let context = Arc::clone(&context);
        let stop = Arc::clone(&stop);
        let workers = Arc::clone(&workers);
        let spawned = thread::Builder::new()
            .name("vox-mcp-accept".to_string())
            .spawn(move || accept_loop(listener, &context, &stop, &workers));
        match spawned {
            Ok(accept) => accept,
            Err(error) => {
                // 线程起不来 = 服务没起来：把刚写的握手文件收回去，别留一个指向不存在端口的凭据。
                remove_handshake_if_ours(&state_file, &token);
                return Err(error);
            }
        }
    };

    // 资源变更的 ticker：**全服务一条**（设计稿 §2.4.2）。没有订阅流时它不碰账本，
    // 只睡在条件变量上等被叫醒（新订阅 / 停服）或到点再看一眼。
    let watch = {
        let context = Arc::clone(&context);
        let stop = Arc::clone(&stop);
        thread::Builder::new()
            .name("vox-mcp-watch".to_string())
            .spawn(move || watch_loop(&context, &stop))
            .ok()
    };

    Ok(ServerHandle {
        addr,
        token,
        state_file,
        stop,
        workers,
        accept: Some(accept),
        watch,
        wake,
    })
}

/// 服务端共享状态。`options`/`token` 只读；`backend`、`slots`、`streams` 要跨连接线程。
struct Context {
    options: ServerOptions,
    token: String,
    backend: Mutex<Option<BoxedBackend>>,
    slots: Slots,
    /// 登记在册的订阅流（ticker 按它们的过滤投递）。
    streams: Mutex<Vec<StreamSub>>,
    /// 连接内唯一的登记号（关流时按它摘掉自己——两条连接可能用同一个 JSON-RPC id）。
    next_stream: AtomicU64,
    wake: Arc<Wake>,
}

/// ticker 的唤醒器：停服与新订阅都要**立刻**叫醒它（否则最长要等一个 notify 周期）。
#[derive(Default)]
struct Wake {
    lock: Mutex<()>,
    condvar: Condvar,
}

impl Wake {
    /// 叫醒 ticker。锁中毒不影响这件事（我们只是要一个"醒"的信号）。
    fn wake(&self) {
        let _guard = lock(&self.lock);
        self.condvar.notify_all();
    }

    /// 睡 `timeout`，或者被 [`Wake::wake`] 叫醒。
    fn sleep(&self, timeout: Duration) {
        let guard = lock(&self.lock);
        let _ = self
            .condvar
            .wait_timeout(guard, timeout)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

/// 一条登记在册的订阅流。ticker 只认三件事：登记号、协议层算好的过滤、消息去处。
struct StreamSub {
    token: u64,
    subscription: Subscription,
    sink: Sender<Value>,
}

/// 资源变更的 ticker：**唯一**一处"什么时候发通知"。
///
/// 它自己不认识任何资源语义——`poll_resources` 给什么就投什么，按每条流自己的过滤投
/// （过滤是协议层 [`Subscription`] 的事）。没有订阅流时它连账本都不碰。
///
/// **总闸**（[`ControlBackend::control_enabled`]）也在这里读，而且每拍都读：`control.enabled`
/// 一关，在册的订阅流**必须被收掉**（S1 稿的承诺），不是静默挂着。收流走规范那条干净结束的
/// 路——每条流先发一条 `result`（`resultType: "complete"` + 同一个 `subscriptionId`）——
/// 客户端据此区分"服务端主动收"与"意外断开"。
fn watch_loop(context: &Arc<Context>, stop: &Arc<AtomicBool>) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }

        let waiting = lock(&context.streams).is_empty();
        if waiting {
            context.wake.sleep(Duration::from_millis(IDLE_WATCH_MS));
            continue;
        }

        match gate(context) {
            // 总闸关着：把在册的流全收掉，然后下一轮就走进上面那条"没有流"的路（连账本都不碰）。
            Gate::Off => close_streams(context),
            // 没有后端就没有订阅流（`subscriptions/listen` 会先回一条错误）；睡一觉再看。
            Gate::NoBackend => context.wake.sleep(Duration::from_millis(IDLE_WATCH_MS)),
            Gate::On => {
                let tick = {
                    let mut backend = lock(&context.backend);
                    backend.as_mut().map(|backend| backend.poll_resources())
                };
                let Some(tick) = tick else {
                    context.wake.sleep(Duration::from_millis(IDLE_WATCH_MS));
                    continue;
                };

                deliver(context, &tick);
                // 下一拍等多久是**后端现读**的设置（`Settings.control.transcript_notify_ms`），
                // 所以用户在设置里改了间隔，下一个 tick 就生效。
                context
                    .wake
                    .sleep(Duration::from_millis(u64::from(tick.notify_ms.max(1))));
            }
        }
    }
}

/// 总闸（`Settings.control.enabled`）与后端的现况。三个取值，因为"没有后端"不能当成"关着"：
/// 后者要收流，前者根本开不出流来。
enum Gate {
    On,
    Off,
    NoBackend,
}

fn gate(context: &Context) -> Gate {
    let mut backend = lock(&context.backend);
    // 不写成 `match` 的 guard：guard 里拿不到可变借用（`control_enabled` 要 `&mut self`）。
    let Some(backend) = backend.as_mut() else {
        return Gate::NoBackend;
    };
    if backend.control_enabled() {
        Gate::On
    } else {
        Gate::Off
    }
}

/// 收掉**全部**在册的订阅流：每条先发一条"干净结束"的 result，再把它的接收端丢掉
/// （`Conn::pump` 写完那条 result 就会看到 `Disconnected` 收工，连接随之关闭）。
///
/// 这就是"总闸关掉"的可观察语义：客户端拿到一条 result 而不是一个永远静着的连接。
fn close_streams(context: &Context) {
    let mut streams = lock(&context.streams);
    for stream in streams.drain(..) {
        let _ = stream.sink.send(stream.subscription.closed());
    }
}

/// 把这一拍的变化按每条流自己的过滤投出去；对端已经走了的流顺手摘掉（它的接收端已经关了）。
fn deliver(context: &Context, tick: &ResourceTick) {
    let mut streams = lock(&context.streams);
    streams.retain_mut(|stream| {
        stream
            .subscription
            .updates(tick)
            .into_iter()
            .all(|message| stream.sink.send(message).is_ok())
    });
}

/// 并发名额。满了就在 [`Slots::acquire`] 里等（内核的 backlog 顶着新连接），不丢请求。
struct Slots {
    free: Mutex<usize>,
    ready: Condvar,
}

impl Slots {
    fn new(total: usize) -> Self {
        Self {
            free: Mutex::new(total.max(1)),
            ready: Condvar::new(),
        }
    }

    /// 取一个名额；`stop` 置位时立刻放弃——关机时卡在这里的接受循环靠这条退出。
    fn acquire(&self, stop: &AtomicBool) -> bool {
        let mut free = lock(&self.free);
        while *free == 0 {
            if stop.load(Ordering::SeqCst) {
                return false;
            }
            let (guard, _) = self
                .ready
                .wait_timeout(free, Duration::from_millis(POLL_MS))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            free = guard;
        }
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        *free -= 1;
        true
    }

    fn release(&self) {
        let mut free = lock(&self.free);
        *free += 1;
        self.ready.notify_one();
    }
}

/// 名额的归还靠 Drop：连接线程无论怎么结束，名额都不会漏。
struct SlotGuard<'a>(&'a Slots);

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// 接受循环：每连接一个线程。线程起不来就把名额还回去，别把池子耗干。
fn accept_loop(
    listener: TcpListener,
    context: &Arc<Context>,
    stop: &Arc<AtomicBool>,
    workers: &Arc<Mutex<Vec<JoinHandle<()>>>>,
) {
    for incoming in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = incoming else {
            // 接受出错多半是瞬时的（`ECONNABORTED` 之类）；睡一拍再试，别把 CPU 烧在紧循环里。
            thread::sleep(Duration::from_millis(POLL_MS));
            continue;
        };
        if !context.slots.acquire(stop) {
            break;
        }
        let thread_context = Arc::clone(context);
        let thread_stop = Arc::clone(stop);
        let spawned = thread::Builder::new()
            .name("vox-mcp-conn".to_string())
            .spawn(move || {
                let _slot = SlotGuard(&thread_context.slots);
                if let Err(error) = serve_connection(stream, &thread_context, &thread_stop) {
                    report_error(&error);
                }
            });
        match spawned {
            Ok(worker) => lock(workers).push(worker),
            Err(_) => context.slots.release(),
        }
    }
}

/// 一条连接上的一串请求（HTTP/1.1 keep-alive；`Connection: close` 或空闲 60 s 才收工）。
fn serve_connection(stream: TcpStream, context: &Context, stop: &AtomicBool) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(POLL_MS)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let idle = Duration::from_millis(u64::from(context.options.idle_timeout_ms));
    let mut conn = Conn {
        stream,
        inbox: Vec::new(),
        last_read: Instant::now(),
    };

    loop {
        let head = match conn.read_head(stop, idle, context.options.max_header_bytes)? {
            Head::Request(bytes) => match ParsedHead::parse(&bytes) {
                Some(head) => head,
                None => return conn.reject(Response::status(400)),
            },
            Head::Closed => return Ok(()),
            Head::TooLarge => return conn.reject(Response::status(431)),
        };

        if head.path() != PATH {
            return conn.reject(Response::status(404));
        }
        if head.method != "POST" {
            return conn.reject(Response::status(405).header("Allow", "POST"));
        }
        // Origin：**任何**带 Origin 头的请求一律 403（设计稿 §2.5.1：规范 MUST 校验，我们取最严）。
        // 代价写在设计稿里：浏览器版 MCP Inspector 因此用不了，用 CLI 版。
        if head.header("origin").is_some() {
            return conn.reject(Response::status(403));
        }
        // 鉴权在**读体之前**：没凭据的请求不该让我们先花内存把体收下来。
        if !authorized(&head, &context.token) {
            return conn.reject(Response::status(401));
        }

        let length = match body_length(&head) {
            Ok(length) => length,
            Err(response) => return conn.reject(response),
        };
        if length > context.options.max_body_bytes {
            return conn.reject(Response::status(413));
        }
        // `Expect: 100-continue`：客户端在等我们点头才发体（curl 对 >1 KiB 的体会带这个头）。
        // 不点头就是死锁——两边都在等对方。
        if head.expects_continue() {
            conn.stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
            conn.stream.flush()?;
        }
        let Some(body) = conn.read_body(length, stop, idle)? else {
            return Ok(()); // 对端没把体送完就断了
        };

        let reply = respond(&head, &body, context);
        let mut response = match reply {
            Reply::One(response) => response,
            // 长流：这条连接从这一刻起归它，直到客户端断开或服务端收流。
            Reply::Stream(subscription) => return conn.serve_stream(subscription, context, stop),
        };
        response.close = response.close || head.wants_close();
        let close = response.close;
        conn.reply(response)?;
        if close {
            return Ok(());
        }
    }
}

/// 一条消息的处理结果：要么一条普通 HTTP 响应，要么一条 SSE 长流。
enum Reply {
    One(Response),
    Stream(Subscription),
}

/// 一条消息 → 一条响应（或一条长流）。**协议语义全在 [`crate::handle`] 里**，这里只做传输层
/// 的三件事：请求头与 body 的一致性校验（`-32020`）、JSON-RPC 错误码到 HTTP 状态码的映射、
/// 以及"这条回答是不是一条流"。
fn respond(head: &ParsedHead, body: &[u8], context: &Context) -> Reply {
    let message: Value = match serde_json::from_slice(body) {
        Ok(message) => message,
        Err(_) => {
            let error = ErrorObject::new(code::PARSE_ERROR, "请求体不是合法 JSON");
            return Reply::One(Response::json(
                400,
                jsonrpc::error_result(None, &error).to_string(),
            ));
        }
    };

    if let Some(error) = header_mismatch(head, &message) {
        let id = id_of(&message);
        return Reply::One(Response::json(
            400,
            jsonrpc::error_result(id.as_ref(), &error).to_string(),
        ));
    }

    match dispatch(&message, context) {
        // 通知：规范要求 202 Accepted 且无 body（`streamable-http#sending-messages`）。
        // **不关连接**：通知与请求会在同一条连接上交替（客户端先发通知再发请求是常态）。
        Answer::Silence => Reply::One(Response::accepted()),
        Answer::Response(response) => {
            let status = http_status(&response);
            Reply::One(Response::json(status, response.to_string()))
        }
        // `subscriptions/listen`：这条请求的"回答"是一条长流，不是一条消息。
        Answer::Stream(subscription) => Reply::Stream(subscription),
    }
}

/// 唯一的后端入口。后端是**同步阻塞**的（[`ControlBackend`](crate::handlers::ControlBackend) 的
/// 契约），所以这里拿一把锁把它串起来——并发 8 条连接共用一台设备，串行执行才是对的。
fn dispatch(message: &Value, context: &Context) -> Answer {
    let mut backend = lock(&context.backend);
    match backend.as_mut() {
        Some(backend) => crate::handle(message, Some(backend.as_mut())),
        None => crate::handle(message, None),
    }
}

/// `Mcp-*` 请求头与 body 的一致性（规范：不符 → 400 + `-32020`）。
///
/// 三条规则（`streamable-http#request-metadata`）：
///
/// - `MCP-Protocol-Version` **必带**，且与 `_meta.protocolVersion` **一致**。body 里根本没有
///   `protocolVersion` 时不比——那是 `_meta` 缺必填，交给协议层报 `-32602`（比"头不符"准得多）。
/// - `Mcp-Method` **必带**，且与 body 的 `method` 一致。body 读不出 `method` 时不比，
///   同样交给帧层报 `-32600`。
/// - `Mcp-Name`（`tools/call` / `resources/read`）**必带**，且与 body 里的名字一致。
///
/// 三个值都**先过 Base64 sentinel 解码再比**（规范：头值只能放 ASCII 白名单，非 ASCII 的值
/// 客户端会包成 `=?base64?<base64>?=`；服务端 MUST 先解码）。
fn header_mismatch(head: &ParsedHead, body: &Value) -> Option<ErrorObject> {
    let body_version = body
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| meta.get(meta::key::PROTOCOL_VERSION))
        .and_then(Value::as_str);

    let version_header = match decoded(head, HEADER_PROTOCOL_VERSION) {
        Ok(header) => header,
        Err(error) => return Some(error),
    };
    match version_header {
        None => {
            return Some(mismatch(
                HEADER_PROTOCOL_VERSION,
                "每请求必带",
                body_version,
                None,
            ))
        }
        Some(header) => {
            if let Some(body_version) = body_version {
                if header != body_version {
                    return Some(mismatch(
                        HEADER_PROTOCOL_VERSION,
                        "必须与 body 的 _meta.protocolVersion 一致",
                        Some(body_version),
                        Some(&header),
                    ));
                }
            }
        }
    }

    // body 读不出 `method` 时不比，交给帧层报 `-32600`（"不是合法 JSON-RPC 请求"比"头不符"准）。
    let method = body.get("method").and_then(Value::as_str)?;
    let method_header = match decoded(head, HEADER_METHOD) {
        Ok(header) => header,
        Err(error) => return Some(error),
    };
    match method_header {
        None => return Some(mismatch(HEADER_METHOD, "每请求必带", Some(method), None)),
        Some(header) if header != method => {
            return Some(mismatch(
                HEADER_METHOD,
                "必须与 body 的 method 一致",
                Some(method),
                Some(&header),
            ))
        }
        Some(_) => {}
    }

    // `Mcp-Name`：规范的表里是 `tools/call` / `resources/read` / `prompts/get` 三条
    // （`params.name` 或 `params.uri`），我们实现了前两条。
    if let Some((what, body_name)) = named_request(body, method) {
        let name_header = match decoded(head, HEADER_NAME) {
            Ok(header) => header,
            Err(error) => return Some(error),
        };
        match name_header {
            None => return Some(mismatch(HEADER_NAME, what, body_name, None)),
            Some(header) if Some(header.as_ref()) != body_name => {
                return Some(mismatch(HEADER_NAME, what, body_name, Some(&header)))
            }
            Some(_) => {}
        }
    }

    None
}

/// 这条方法要不要 `Mcp-Name`，以及它该等于 body 里的哪一格。
fn named_request<'a>(body: &'a Value, method: &str) -> Option<(&'static str, Option<&'a str>)> {
    match method {
        mcp::METHOD_TOOLS_CALL => Some((
            "tools/call 必带，且必须与 body 的 params.name 一致",
            body.pointer("/params/name").and_then(Value::as_str),
        )),
        mcp::METHOD_RESOURCES_READ => Some((
            "resources/read 必带，且必须与 body 的 params.uri 一致",
            body.pointer("/params/uri").and_then(Value::as_str),
        )),
        _ => None,
    }
}

/// 规范要求的三个请求头。查的时候按小写（HTTP 头名不分大小写），报错时按这里的拼写。
const HEADER_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
const HEADER_METHOD: &str = "Mcp-Method";
const HEADER_NAME: &str = "Mcp-Name";

/// 取一个请求头并解掉 Base64 sentinel。`Ok(None)` = 头不在。
fn decoded<'a>(head: &'a ParsedHead, name: &str) -> Result<Option<Cow<'a, str>>, ErrorObject> {
    let Some(raw) = head.header_any_case(name) else {
        return Ok(None);
    };
    match decode_sentinel(raw) {
        Ok(value) => Ok(Some(value)),
        Err(()) => Err(mismatch(
            name,
            "Base64 sentinel（`=?base64?<base64>?=`）写坏了",
            None,
            Some(raw),
        )),
    }
}

/// 头值的 Base64 sentinel 解码（`=?base64?<base64>?=`）。
///
/// 没写 sentinel 的值原样返回；写了却解不出来是**坏头**，调用方要回 `-32020`。
fn decode_sentinel(value: &str) -> Result<Cow<'_, str>, ()> {
    let Some(rest) = value.strip_prefix("=?base64?") else {
        return Ok(Cow::Borrowed(value));
    };
    let Some(payload) = rest.strip_suffix("?=") else {
        return Err(());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| ())?;
    String::from_utf8(bytes).map(Cow::Owned).map_err(|_| ())
}

/// JSON-RPC 错误码 → HTTP 状态码。
///
/// 规范只钉了三种：请求头不符 / `_meta` 问题 / 版本不支持 → 400，未知方法 → 404
/// （`streamable-http` 的 `#server-validation` 与 `#protocol-version-header`）。
/// 其余按 HTTP 的语义就近取：内部错误 500，解析/帧错误 400。成功的工具调用是 200
/// （**领域失败也是 200**：它在 `structuredContent.error` 里，是给模型看的，不是给传输层看的）。
fn http_status(response: &Value) -> u16 {
    match response
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_i64)
    {
        None => 200,
        Some(number) if number == i64::from(code::METHOD_NOT_FOUND) => 404,
        Some(number) if number == i64::from(code::INTERNAL_ERROR) => 500,
        Some(_) => 400,
    }
}

fn id_of(message: &Value) -> Option<Value> {
    message
        .get("id")
        .filter(|id| id.is_string() || id.is_number())
        .cloned()
}

fn mismatch(
    header: &str,
    why: &str,
    expected: Option<&str>,
    received: Option<&str>,
) -> ErrorObject {
    ErrorObject::with_data(
        code::HEADER_MISMATCH,
        format!("{header} 请求头不合法：{why}"),
        json!({ "header": header, "expected": expected, "received": received }),
    )
}

/// 体的分帧（只认 `Content-Length`）。
fn body_length(head: &ParsedHead) -> Result<usize, Response> {
    if let Some(encoding) = head.header("transfer-encoding") {
        if !encoding.eq_ignore_ascii_case("identity") {
            // 分块体要自己写解码器（连 trailer 一起）。本版规范里没有客户端会这么发，
            // 与其半吊子支持一种畸形输入，不如明确拒绝。
            return Err(Response::status(501));
        }
    }
    match head.header("content-length") {
        Some(text) => text
            .trim()
            .parse::<usize>()
            .map_err(|_| Response::status(400)),
        None => Err(Response::status(411)),
    }
}

/// `Authorization: Bearer <token>`，常数时间比较（设计稿 §2.5.1）。
///
/// 常数时间指的是**内容**比较：长度不同会立刻返回，但那不是秘密（token 长度是公开的常量）。
fn authorized(head: &ParsedHead, token: &str) -> bool {
    let Some(value) = head.header("authorization") else {
        return false;
    };
    let Some(presented) = bearer(value) else {
        return false;
    };
    constant_time_eq(presented.as_bytes(), token.as_bytes())
}

fn bearer(value: &str) -> Option<&str> {
    let (scheme, rest) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(a ^ b);
    }
    diff == 0
}

/// 32 字节随机数 → 43 字符 base64url（无填充）。
fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| io::Error::other(format!("取随机数失败：{error}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// 握手文件：`{"port":47123,"token":"<43 字符>","pid":12345,"protocolVersion":"2026-07-28"}`
/// （设计稿 §2.5.1 逐字）。先写临时文件再 rename，免得 CLI 读到半截 JSON。
fn write_handshake(path: &Path, port: u16, token: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    let document = json!({
        "port": port,
        "token": token,
        "pid": std::process::id(),
        "protocolVersion": meta::PROTOCOL_VERSION,
    })
    .to_string();

    // 临时文件与目标同目录：rename 才是原子的（跨设备 rename 会失败）。
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut file = create_owner_only(&temp)?;
        file.write_all(document.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&temp, path)
}

/// 握手文件里的**地址与凭据**（就是 [`write_handshake`] 写出来的那个文档）。
///
/// 读的一侧只有一个消费者：CLI / stdio 桥那条"连本机控制面"的瘦客户端
/// （[`crate::client::ControlPlane`]）。所以判据按**消费者要什么**定：`port` 与 `token`
/// 缺一不可；`pid` / `protocolVersion` 是给人和排错看的，不在这里二次校验——凭据真错了，
/// 上游会拿 401 说话，那比"我们猜它过期了"准。
///
/// 地址**不读文件里的任何格**：控制面只绑回环（[`serve`] 会拒绝别的地址），所以调用方拿
/// `127.0.0.1:port` 就行；往握手文件里再存一份 ip 只会多一格可能对不上的东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub port: u16,
    pub token: String,
}

impl Handshake {
    /// 读一个握手文件。所有失败都是 `InvalidData` + 一句中文原因（调用方只需把它打出来）。
    pub fn read(path: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("读不到握手文件 {}：{error}", path.display()),
            )
        })?;
        let document: Value = serde_json::from_str(&text).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} 不是合法 JSON：{error}", path.display()),
            )
        })?;

        let port = document
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|port| u16::try_from(port).ok());
        let token = document
            .get("token")
            .and_then(Value::as_str)
            .map(str::to_string);
        match (port, token) {
            (Some(port), Some(token)) if !token.is_empty() => Ok(Self { port, token }),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{} 不是握手文件：要 {{\"port\":<1-65535>,\"token\":\"<非空>\"}}，\
                     读到的是 {document}",
                    path.display()
                ),
            )),
        }
    }
}

/// 建文件时就带上 `0600`（设计稿 §2.5.1：Unix 下 `0600`，Windows / Android 靠用户 profile 的 ACL）。
///
/// 这不是"平台能力开关"——本 crate 里能力差异一律来自注入的 `HostFacts`（§2.6）。文件权限
/// 是内核 API，没有跨平台写法，只能在这里 `cfg`。
#[cfg(unix)]
fn create_owner_only(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_owner_only(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

/// 停服时擦掉自己写的握手文件。只有**还是我们那一份**（token 相同）才删——否则会把
/// 后起的那个实例的文件删掉。
fn remove_handshake_if_ours(path: &Path, token: &str) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let ours = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|document| {
            document
                .get("token")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .as_deref()
        == Some(token);
    if ours {
        let _ = fs::remove_file(path);
    }
}

/// 连接错误只报"值得看"的那些：客户端挂断（RST / 半截请求）是日常，不是故障。
fn report_error(error: &io::Error) {
    use io::ErrorKind::*;
    if matches!(
        error.kind(),
        ConnectionReset | ConnectionAborted | BrokenPipe | UnexpectedEof | NotConnected
    ) {
        return;
    }
    eprintln!("vox-mcp: 控制面连接出错：{error}");
}

/// 一条连接：`inbox` 是"已经从内核读进用户态、还没被消费"的字节。
struct Conn {
    stream: TcpStream,
    inbox: Vec<u8>,
    last_read: Instant,
}

/// 读请求头的结果。
enum Head {
    /// 完整的请求头（含结尾的 `\r\n\r\n`）。
    Request(Vec<u8>),
    /// 对端关闭，或空闲超时。
    Closed,
    /// 超过头上限还没读到结尾。
    TooLarge,
}

impl Conn {
    /// 读到 `\r\n\r\n`。读超时只是轮询节拍，空闲时限按 [`Conn::last_read`] 算。
    fn read_head(
        &mut self,
        stop: &AtomicBool,
        idle: Duration,
        max_header_bytes: usize,
    ) -> io::Result<Head> {
        loop {
            if let Some(end) = find(&self.inbox, b"\r\n\r\n") {
                // 上限要在这里也查一次：对端可以一口气把整颗大头塞进内核缓冲区，
                // 那时"读到结尾了"和"超限"会同时成立，必须先判超限。
                if end + 4 > max_header_bytes {
                    return Ok(Head::TooLarge);
                }
                let head: Vec<u8> = self.inbox.drain(..end + 4).collect();
                return Ok(Head::Request(head));
            }
            if self.inbox.len() > max_header_bytes {
                return Ok(Head::TooLarge);
            }
            if !self.read_more(stop, idle)? {
                return Ok(Head::Closed);
            }
        }
    }

    /// 读满 `length` 个字节（可能一部分已经在 [`Conn::inbox`] 里）。
    fn read_body(
        &mut self,
        length: usize,
        stop: &AtomicBool,
        idle: Duration,
    ) -> io::Result<Option<Vec<u8>>> {
        while self.inbox.len() < length {
            if !self.read_more(stop, idle)? {
                return Ok(None);
            }
        }
        Ok(Some(self.inbox.drain(..length).collect()))
    }

    /// 再读一批。`false` = 该收工了（对端关闭 / 空闲超时 / 停服）。
    fn read_more(&mut self, stop: &AtomicBool, idle: Duration) -> io::Result<bool> {
        if Instant::now().duration_since(self.last_read) >= idle {
            return Ok(false);
        }
        let mut chunk = [0u8; 4096];
        match self.stream.read(&mut chunk) {
            Ok(0) => Ok(false),
            Ok(read) => {
                self.inbox.extend_from_slice(&chunk[..read]);
                self.last_read = Instant::now();
                Ok(true)
            }
            // 读超时只是节拍到了：看一眼 stop，继续等。
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(!stop.load(Ordering::SeqCst))
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(true),
            Err(error) => Err(error),
        }
    }

    fn reply(&mut self, response: Response) -> io::Result<()> {
        write_response(&mut self.stream, &response)
    }

    /// 回一条"到此为止"的响应：先把对端还在路上的字节排空再关连接。
    ///
    /// 带着未读数据 `close()` 会让内核发 RST，把已经写出的响应冲掉，客户端看到的是
    /// `connection reset` 而不是我们真实的拒绝理由（401/403/413…）。
    fn reject(&mut self, response: Response) -> io::Result<()> {
        self.reply(response)?;
        let deadline = Instant::now() + Duration::from_millis(POLL_MS);
        let mut scratch = [0u8; 4096];
        while Instant::now() < deadline {
            match self.stream.read(&mut scratch) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        Ok(())
    }

    /// 把这条连接变成一条 SSE 长流（`subscriptions/listen` 的回答），直到对端断开或服务端收流。
    ///
    /// 这条连接从此不再解析 HTTP 请求：规范说得很清楚——HTTP 上**关掉这条流就是取消**，
    /// 没有 `notifications/cancelled`（那是 stdio 的事）。
    fn serve_stream(
        &mut self,
        subscription: Subscription,
        context: &Context,
        stop: &AtomicBool,
    ) -> io::Result<()> {
        // ack 与收尾响应都是纯函数，先算出来：`subscription` 要搬进登记表，之后拿不回来。
        let acknowledged = subscription.acknowledged();
        let closing = subscription.closed();

        self.stream.write_all(SSE_HEAD.as_bytes())?;
        self.stream.flush()?;

        let token = context.next_stream.fetch_add(1, Ordering::SeqCst);
        let (sink, inbox) = mpsc::channel();
        lock(&context.streams).push(StreamSub {
            token,
            subscription,
            sink,
        });
        // 叫醒 ticker：让"订阅之后"的变化尽快投出去。水位在 `mcp::listen` 回 ack 之前就已经
        // 对齐好了（见 `subscriptions::accept`），这里只负责别让第一拍再压后一个周期
        // （登记前 ticker 可能正睡在"没有流"的空转里，或睡在上一个 notify 间隔上）。
        context.wake.wake();

        let outcome = self.pump(&acknowledged, &closing, &inbox, context, stop);
        lock(&context.streams).retain(|stream| stream.token != token);
        outcome
    }

    /// 长流的主循环：收到消息就写，超时就检查"停服没有 / 对端还在不在 / 该不该保活"。
    fn pump(
        &mut self,
        acknowledged: &Value,
        closing: &Value,
        inbox: &Receiver<Value>,
        context: &Context,
        stop: &AtomicBool,
    ) -> io::Result<()> {
        let keep_alive = Duration::from_millis(u64::from(context.options.sse_keep_alive_ms));
        self.write_event(acknowledged)?;
        let mut last_write = Instant::now();

        loop {
            match inbox.recv_timeout(Duration::from_millis(POLL_MS)) {
                Ok(message) => {
                    self.write_event(&message)?;
                    last_write = Instant::now();
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {
                    if stop.load(Ordering::SeqCst) {
                        // 规范 SHOULD：服务端主动收流前先回一条 result，客户端据此知道是
                        // "干净结束"（没有这条就是意外断开，客户端该重连）。
                        self.write_event(closing)?;
                        return Ok(());
                    }
                    if self.client_gone() {
                        return Ok(());
                    }
                    if last_write.elapsed() >= keep_alive {
                        // 规范：长流空闲时发注释行保活（`:` 开头的行不携带任何数据）。
                        self.stream.write_all(b":\r\n")?;
                        self.stream.flush()?;
                        last_write = Instant::now();
                    }
                }
            }
        }
    }

    /// 对端还在吗？这条流上不该再收到任何字节，所以读到的都丢掉——只看 `Ok(0)`（对端关了）。
    fn client_gone(&mut self) -> bool {
        let mut scratch = [0u8; 1024];
        match self.stream.read(&mut scratch) {
            Ok(0) => true,
            Ok(_) => false,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                false
            }
            Err(_) => true,
        }
    }

    /// 一个 SSE 事件：`data: <一行 JSON>`。JSON 里没有换行（`to_string` 不产生换行），
    /// 所以一个事件一行数据就够，不用按 SSE 的规矩拆多行。
    fn write_event(&mut self, message: &Value) -> io::Result<()> {
        let text = message.to_string();
        let mut frame = String::with_capacity(text.len() + 8);
        frame.push_str("data: ");
        frame.push_str(&text);
        frame.push_str("\n\n");
        self.stream.write_all(frame.as_bytes())?;
        self.stream.flush()
    }
}

/// SSE 响应的头。**没有 `Content-Length`**：长流靠关连接划界，所以 HTTP/1.1 下必须
/// `Connection: close`（不是选择，是不这么写客户端就没法知道体到哪儿结束）。
/// `X-Accel-Buffering: no` 是规范 SHOULD（别让反代把事件攒起来）。
const SSE_HEAD: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Cache-Control: no-cache\r\n\
     X-Accel-Buffering: no\r\n\
     Connection: close\r\n\r\n";

/// 解析过的请求头。头名统一小写，比较时不用再管大小写（HTTP 头名不分大小写）。
struct ParsedHead {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
}

impl ParsedHead {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut lines = text.split("\r\n");
        let mut request_line = lines.next()?.split(' ');
        let method = request_line.next()?.to_string();
        let target = request_line.next()?.to_string();
        let version = request_line.next()?.to_string();
        if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/") {
            return None;
        }

        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (name, value) = line.split_once(':')?;
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
        Some(Self {
            method,
            target,
            version,
            headers,
        })
    }

    /// 按小写查（解析时已把名字归一成小写，HTTP 头名不分大小写）。
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    /// 按规范里的拼写查（调用方拿着 `Mcp-Name` 这种名字）。比较不分大小写，也不分配。
    fn header_any_case(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// 路径（去掉 query）。本服务只有一条路径，query 不参与路由。
    fn path(&self) -> &str {
        self.target
            .split_once('?')
            .map_or(self.target.as_str(), |(path, _)| path)
    }

    fn expects_continue(&self) -> bool {
        self.header("expect")
            .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    }

    /// HTTP/1.1 默认长连接；HTTP/1.0 要显式 `Connection: keep-alive`。
    fn wants_close(&self) -> bool {
        let connection = self.header("connection").unwrap_or_default();
        let close = connection
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("close"));
        let keep_alive = connection
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("keep-alive"));
        close || (self.version == "HTTP/1.0" && !keep_alive)
    }
}

/// 一条响应。
struct Response {
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    body: Vec<u8>,
    /// 回完就关连接。
    close: bool,
}

impl Response {
    /// 只有状态码、没有 body 的响应（401 / 403 / 404 / 405 / 202…）。一律关连接：
    /// 拒绝请求时我们没读完对端的体，继续在一条已经错位的连接上解析下一条请求是自找麻烦。
    fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            close: true,
        }
    }

    fn json(status: u16, body: String) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "application/json")],
            body: body.into_bytes(),
            close: false,
        }
    }

    /// `202 Accepted`：通知的回应，空 body、不关连接。
    fn accepted() -> Self {
        Self {
            status: 202,
            headers: Vec::new(),
            body: Vec::new(),
            close: false,
        }
    }

    fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }
}

fn write_response(stream: &mut TcpStream, response: &Response) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for (name, value) in &response.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    head.push_str(if response.close {
        "Connection: close\r\n"
    } else {
        "Connection: keep-alive\r\n"
    });
    head.push_str("\r\n");

    stream.write_all(head.as_bytes())?;
    if !response.body.is_empty() {
        stream.write_all(&response.body)?;
    }
    stream.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Unknown",
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 锁中毒不该让整个服务趴下：被毒到的只有"谁来处理这条连接"这点信息，接着用就是。
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
