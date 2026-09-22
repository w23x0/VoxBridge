//! 连本机控制面的**瘦客户端**：CLI 的动作子命令与 stdio 桥共用这一条路径。
//!
//! 它**不是第二个服务端**（设计稿 §2.2.1）：不注入账本、不开设备、也不实现任何协议语义。
//! 它只做三件事——
//!
//! 1. 读握手文件拿 `port` + `token`（[`Handshake::read`](crate::transport::http::Handshake::read)）；
//! 2. 把一条**已经成形的** JSON-RPC 消息 POST 到 `127.0.0.1:<port>/mcp`，补上规范要求的三个头
//!    （[`mcp_headers`]，与 `transport/http.rs::header_mismatch` 的校验一一对应）；
//! 3. 把上游的响应分成四种交回去（[`Reply`]）。
//!
//! 「CLI 的输出 == MCP 的输出」因此仍然是**同一个 `crate::handle`** 出来的东西：服务端怎么回，
//! 客户端就怎么拿，中间没有第二份实现。唯一的差异是 HTTP 帧本身（`Content-Length`、每连接一个
//! 请求、`text/event-stream` 的 `data:` 行）——那正是这一层**唯一**要懂的东西。
//!
//! 为什么手写而不是引 HTTP 客户端：本 crate 的规矩是**零新依赖**（`Cargo.toml` 的注释 + §2.6），
//! 而这里的形状窄得可以（单路径、POST-only、无重定向、无 cookie、回环地址）——上面那三件事
//! 加起来比接一个客户端库的配置项还短。

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::mcp::{self, meta};
use crate::transport::http::{Handshake, PATH};

/// 连上去之前最多等多久（回环地址上连不上就是没在跑，等它没意义）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 写请求的上限（体本来就小；卡住 = 对端已经不对了）。
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// 等响应头/响应体的上限。**不是**流上每条通知的上限（那是 [`Sse`] 的事，它阻塞等，靠
/// [`Sse::cancel`] 打断）：一次 `session_open` 最坏要等到 `wait_ready_ms` 的上限，这里给得比它宽。
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);

/// 一个跑起来的本机控制面（地址 + 凭据）。
#[derive(Debug, Clone)]
pub struct ControlPlane {
    addr: SocketAddr,
    token: String,
}

impl ControlPlane {
    /// 从握手文件读。地址固定 `127.0.0.1:<port>`：控制面只绑回环
    /// （`serve` 会拒绝别的地址，握手文件里也就不存 ip）。
    pub fn from_state_file(path: &Path) -> io::Result<Self> {
        let handshake = Handshake::read(path)?;
        Ok(Self {
            addr: SocketAddr::from(([127, 0, 0, 1], handshake.port)),
            token: handshake.token,
        })
    }

    /// 听在哪儿（CLI 的报错里会写出来）。
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 把一条消息 POST 上去，取回上游的回答。
    ///
    /// **一条消息一个连接**（`Connection: close`）：控制面是"每请求无状态"的，而命令行的用法
    /// 就是一条命令一条请求；桥那边每条消息也各自成对。省掉连接池，也就省掉了"连接上的东西过期了"
    /// 那一类状态。
    pub fn post(&self, message: &Value) -> io::Result<Reply> {
        self.post_raw(&message.to_string(), message)
    }

    /// 同上，但**线上那串字节由调用方给**（`body`），`message` 只用来推请求头。
    ///
    /// 分成两个入口是给 stdio 桥用的：它要原样转发客户端写下的字节，而不是我们重新序列化一遍
    /// （键序、空白都照原样过去，服务端看到的就是宿主发的）。
    pub fn post_raw(&self, body: &str, message: &Value) -> io::Result<Reply> {
        let mut head = String::with_capacity(256 + body.len());
        head.push_str(&format!("POST {PATH} HTTP/1.1\r\n"));
        head.push_str(&format!("Host: {}\r\n", self.addr));
        head.push_str("Content-Type: application/json\r\n");
        head.push_str("Accept: application/json, text/event-stream\r\n");
        // 不带 `Origin`：控制面对**任何**带 Origin 的请求一律 403（§2.5.1），我们是本机客户端。
        for (name, value) in mcp_headers(message) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!("Authorization: Bearer {}\r\n", self.token));
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        head.push_str("Connection: close\r\n\r\n");

        let mut stream = TcpStream::connect_timeout(&self.addr, CONNECT_TIMEOUT)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        stream.set_read_timeout(Some(RESPONSE_TIMEOUT))?;
        stream.write_all(head.as_bytes())?;
        stream.write_all(body.as_bytes())?;
        stream.flush()?;

        let (status, content_type, inbox) = read_head(&mut stream)?;
        if content_type
            .as_deref()
            .is_some_and(|value| value.contains("text/event-stream"))
        {
            // 长流：**不带** `Content-Length`，靠关连接划界（服务端就是这么写的）。之后的事件
            // 一条条取，读超时在这里不设——阻塞等，取消靠 [`Sse::cancel`] 打断。
            return Ok(Reply::Stream(Sse::new(stream, inbox)?));
        }

        let body = read_to_end(&mut stream, inbox)?;
        if body.is_empty() {
            return Ok(if status == 202 {
                // 通知的回应：规范要求 202 且无 body。什么都不用交回去。
                Reply::Accepted
            } else {
                Reply::Unexpected {
                    status,
                    body: String::new(),
                }
            });
        }
        match serde_json::from_slice(&body) {
            Ok(message) => Ok(Reply::Message(message)),
            Err(_) => Ok(Reply::Unexpected {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            }),
        }
    }
}

/// 上游对一条消息的回答。
#[derive(Debug)]
pub enum Reply {
    /// 一条 JSON-RPC 消息：成功结果或错误响应（**包括** `-32602` / `-32603` 那类——HTTP 400/500
    /// 与 JSON-RPC 错误码的映射是服务端的事，客户端不重复解释一遍）。领域失败（`isError`）
    /// 也在这一格里：它是工具结果，规范要求它走 200。
    Message(Value),
    /// `202` + 空 body：通知的回应（规范：接收方不回通知）。
    Accepted,
    /// 一条 SSE 长流（`subscriptions/listen`）。
    Stream(Sse),
    /// 上游回了看不懂的东西：既不是 JSON、也不是空 body 的那几种（401/403/404 没有 body，
    /// 或者代理塞进来的 HTML）。**如实报错，不编造协议内容**。
    Unexpected { status: u16, body: String },
}

/// 一条 SSE 长流的上游半边。
///
/// 只有两件事：逐条取 `data:` 负载，以及"取消"（HTTP 上关掉这条流就是取消，规范没有
/// `notifications/cancelled`）。
#[derive(Debug)]
pub struct Sse {
    stream: TcpStream,
    inbox: Vec<u8>,
}

impl Sse {
    fn new(stream: TcpStream, inbox: Vec<u8>) -> io::Result<Self> {
        // 事件之间可能隔很久（空闲时服务端每 15 s 发一个保活注释行）——这里阻塞等，
        // 不等同于"上游死了"：真断了会读到 0，取消会把它打断。
        stream.set_read_timeout(None)?;
        Ok(Self { stream, inbox })
    }

    /// 下一条消息（`data:` 行的 JSON）。`:` 保活注释行与空行跳过；`Ok(None)` = 流结束了。
    pub fn next_message(&mut self) -> io::Result<Option<Value>> {
        loop {
            if let Some(end) = find(&self.inbox, b"\n") {
                let line: Vec<u8> = self.inbox.drain(..end + 1).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim_end_matches(['\r', '\n']);
                if line.is_empty() {
                    continue; // 事件之间的分隔行
                }
                // 规范：`:` 开头的是保活注释（不携带数据）；别的前缀也不该被当成消息，跳过。
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim_start();
                if data.is_empty() {
                    continue; // 空负载（`data:` 后面什么都没有）不是一条消息
                }
                return serde_json::from_str(data).map(Some).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("SSE 事件体不是 JSON（{error}）：{data}"),
                    )
                });
            }

            let mut chunk = [0u8; 4096];
            let read = self.stream.read(&mut chunk)?;
            if read == 0 {
                return Ok(None);
            }
            self.inbox.extend_from_slice(&chunk[..read]);
        }
    }

    /// 拿一个句柄给"取消"用：阻塞在 `read` 上的那条线程会被 [`Sse::cancel`] 叫醒。
    pub fn handle(&self) -> io::Result<TcpStream> {
        self.stream.try_clone()
    }

    /// 取消这条流：关掉上游那条 POST 的连接（服务端看到对端走了就收摊）。
    pub fn cancel(&self) -> io::Result<()> {
        self.stream.shutdown(Shutdown::Both)
    }
}

/// 规范对请求头的要求：`MCP-Protocol-Version` 每请求必带且与 body 的 `_meta` 一致；
/// `Mcp-Method` 必带且与 `method` 一致；`Mcp-Name` 在 `tools/call` / `resources/read` 上必带，
/// 值 = `params.name` / `params.uri`（规范 `streamable-http#request-metadata`）。
///
/// 值**只从消息本身推**，一个字都不加工：这样"服务端看到的就是客户端发的"，头与 body 不可能
/// 各说各话。`_meta` 里没有版本时用本服务端讲的那个（那是给"手写 curl 忘了带"准备的，
/// 协议层仍会按缺必填回 `-32602`）。
fn mcp_headers(message: &Value) -> Vec<(&'static str, String)> {
    let meta_object = message.pointer("/params/_meta").and_then(Value::as_object);
    let version = meta_object
        .and_then(|meta| meta.get(meta::key::PROTOCOL_VERSION))
        .and_then(Value::as_str)
        .unwrap_or(meta::PROTOCOL_VERSION);

    let mut headers = vec![("MCP-Protocol-Version", version.to_string())];
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // 连方法都读不出来：那是坏帧，交给服务端的帧层报 `-32600`／`-32020`，
        // 这里不替它猜（头少一个，服务端会照实说缺哪个）。
        return headers;
    };
    headers.push(("Mcp-Method", method.to_string()));

    let name = match method {
        mcp::METHOD_TOOLS_CALL => message.pointer("/params/name"),
        mcp::METHOD_RESOURCES_READ => message.pointer("/params/uri"),
        _ => None,
    };
    if let Some(name) = name.and_then(Value::as_str) {
        headers.push(("Mcp-Name", name.to_string()));
    }
    headers
}

/// 读响应头。返回 `(状态码, Content-Type, 已经读进用户态但还没被消费的字节)`。
///
/// **不做 chunked 解码**：控制面从不发分块体（`transport/http.rs::write_response` 只有
/// `Content-Length` 与"关连接划界"两种）。
fn read_head(stream: &mut TcpStream) -> io::Result<(u16, Option<String>, Vec<u8>)> {
    let mut inbox: Vec<u8> = Vec::new();
    let end = loop {
        if let Some(end) = find(&inbox, b"\r\n\r\n") {
            break end;
        }
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "控制面在回完整响应头之前就断了连接",
            ));
        }
        inbox.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8(inbox.drain(..end + 4).collect())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "响应头不是 UTF-8"))?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "响应没有状态行"))?;

    let content_type = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim().to_string());

    Ok((status, content_type, inbox))
}

/// 读到连接关闭为止（我们发的是 `Connection: close`，服务端回完就关）。
fn read_to_end(stream: &mut TcpStream, mut inbox: Vec<u8>) -> io::Result<Vec<u8>> {
    loop {
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(inbox);
        }
        inbox.extend_from_slice(&chunk[..read]);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
