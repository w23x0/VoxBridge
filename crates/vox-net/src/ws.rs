//! channel-based 同步↔异步桥接 WebSocket 传输层。
//!
//! 架构：`connect()` 时在 tokio runtime 上 spawn 一个读循环任务（`reader_task`），
//! 它持有 `WebSocketStream` 的读半边，把收到的帧通过 `mpsc` 推给主线程；
//! 写半边留在主线程侧通过 `block_on` 发送（写不需要超时，且 DashScope 不会
//! 背压到阻塞写的地步）。`recv` 用 `recv_timeout` 等 channel，超时时连接不受影响。

use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream, StreamExt};
use futures_util::SinkExt;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use vox_core::cloud::{ConnectRequest, Incoming, Transport};
use vox_core::ports::{PortError, PortResult};

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = SplitSink<WsStream, Message>;
type WsReader = SplitStream<WsStream>;

/// 下行缓冲上限。512 条足够容纳数秒突发译音，同时能阻止断开的 UI/工作线程
/// 让无界队列一直吃内存。满了时读任务自然向 TCP 施加背压。
const READER_QUEUE_SIZE: usize = 512;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// 读循环向主线程投递的消息。
enum ReaderMsg {
    /// 一条文本帧（或从 binary 帧 lossy 解码的文本）。
    Text(String),
    /// 对端发了 close frame。
    Closed(String),
    /// 读循环自己炸了（网络断了之类）。
    Error(String),
}

/// 连接建立后的活跃状态。
struct ActiveConn {
    write: WsSink,
    rx: mpsc::Receiver<ReaderMsg>,
    shutdown: CancellationToken,
}

/// `vox_core::cloud::Transport` 的 WebSocket 实现。
///
/// 线程安全：本身是 `Send`（trait 要求），但不是 `Sync`——同一时刻只有一个
/// 流水线线程在用它。
pub struct WsTransport {
    handle: Handle,
    /// 自己起的 runtime（standalone 模式）。持有它只为保活，不直接用。
    _owned_rt: Option<Arc<Runtime>>,
    conn: Option<ActiveConn>,
}

impl WsTransport {
    /// 复用已有 runtime（Tauri 场景下 app 已经有一个在跑的 tokio runtime）。
    pub fn new(handle: Handle) -> Self {
        ensure_crypto_provider();
        Self {
            handle,
            _owned_rt: None,
            conn: None,
        }
    }

    /// 自己起一个小 runtime，适合测试和 CLI 场景。
    pub fn standalone() -> PortResult<Self> {
        ensure_crypto_provider();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| PortError::new(format!("无法启动网络运行时：{e}")))?;
        let handle = rt.handle().clone();
        let rt = Arc::new(rt);
        Ok(Self {
            handle,
            _owned_rt: Some(rt),
            conn: None,
        })
    }

    /// 如果还有连接就关掉，为下一次 `connect` 腾地方。
    fn teardown(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.shutdown.cancel();
            // 尝试发 close frame，但不等太久——对端可能已经消失了。
            let sink = conn.write;
            let handle = self.handle.clone();
            // spawn 是 fire-and-forget，不需要 await JoinHandle。
            drop(handle.spawn(async move {
                let mut sink = sink;
                let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
            }));
        }
    }
}

impl Transport for WsTransport {
    fn connect(&mut self, request: &ConnectRequest) -> PortResult<()> {
        // 先关掉旧连接（如果有的话）。
        self.teardown();

        // 构造带 Authorization 头的 HTTP 请求——直接传 URL 字符串会丢自定义头，
        // 必须用 tungstenite 的 Request builder。（这是个常见坑）
        let mut req = request
            .url
            .as_str()
            .into_client_request()
            .map_err(|e| PortError::new(format!("URL 格式错误：{e}")))?;
        if !request.auth_header.is_empty() {
            req.headers_mut().insert(
                "Authorization",
                request
                    .auth_header
                    .parse()
                    .map_err(|e| PortError::new(format!("鉴权头格式错误：{e}")))?,
            );
        }

        let safe_url = request.url.split("?key=").next().unwrap_or(&request.url);
        debug!(url = %safe_url, "正在连接 WebSocket");

        // 在 runtime 上执行握手，阻塞当前线程等结果。
        let stream: WsStream = self
            .handle
            .block_on(async {
                // 30 秒连接超时——DNS 解析 + TCP 三次握手 + TLS 协商 + HTTP 升级。
                tokio::time::timeout(
                    Duration::from_secs(30),
                    tokio_tungstenite::connect_async(req),
                )
                .await
            })
            .map_err(|_| PortError::new("连接超时（30 秒内未完成握手）"))?
            .map_err(map_connect_error)?
            .0;

        // 实时音频是频繁的小帧；关闭 Nagle，避免小包在等待 ACK 时平白多挨一拍。
        if let Err(error) = stream.get_ref().get_ref().set_nodelay(true) {
            debug!(%error, "TCP_NODELAY 设置失败，继续使用系统默认行为");
        }

        let (write, read) = stream.split();

        let (tx, rx) = mpsc::channel(READER_QUEUE_SIZE);
        let shutdown = CancellationToken::new();

        // spawn 读循环。
        let token = shutdown.clone();
        self.handle.spawn(reader_task(read, tx, token));

        self.conn = Some(ActiveConn {
            write,
            rx,
            shutdown,
        });

        debug!("WebSocket 握手成功");
        Ok(())
    }

    fn send(&mut self, text: &str) -> PortResult<()> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| PortError::new("发送失败：连接未建立"))?;

        let msg = Message::Text(text.to_owned().into());
        self.handle
            .block_on(async { tokio::time::timeout(WRITE_TIMEOUT, conn.write.send(msg)).await })
            .map_err(|_| PortError::new("发送超时（5 秒内 socket 未能写出）"))?
            .map_err(|e| PortError::new(format!("发送失败：{e}")))?;
        Ok(())
    }

    fn recv(&mut self, timeout_ms: u32) -> PortResult<Option<Incoming>> {
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| PortError::new("接收失败：连接未建立"))?;

        if timeout_ms == 0 {
            return match conn.rx.try_recv() {
                Ok(msg) => Ok(Some(reader_msg(msg))),
                Err(mpsc::error::TryRecvError::Empty) => Ok(None),
                Err(mpsc::error::TryRecvError::Disconnected) => Ok(Some(Incoming::Closed(
                    "连接意外断开（读循环已退出）".to_owned(),
                ))),
            };
        }

        // 用 tokio 的 timeout 等 channel——超时时 channel 和读循环都不受影响。
        let dur = Duration::from_millis(timeout_ms as u64);
        let result = self
            .handle
            .block_on(async { tokio::time::timeout(dur, conn.rx.recv()).await });

        match result {
            // 超时——安静路径，连接还活着。
            Err(_elapsed) => Ok(None),
            // channel 关了（读循环退出了但没来得及发 Error/Closed）。
            Ok(None) => Ok(Some(Incoming::Closed(
                "连接意外断开（读循环已退出）".to_owned(),
            ))),
            Ok(Some(msg)) => Ok(Some(reader_msg(msg))),
        }
    }

    fn close(&mut self) {
        self.teardown();
    }
}

// --- 读循环 ------------------------------------------------------------------

/// 确保进程级 CryptoProvider 已安装。rustls 0.23 需要显式选一个 provider
/// （aws-lc-rs 或 ring），否则首次 TLS 握手时 panic。装过就跳过（幂等）。
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// 从 WebSocket 读帧、过滤 Ping/Pong、投递给主线程。
///
/// tungstenite 0.30 的客户端**会自动回复 Ping**（在 `read()` 内部处理的），
/// 所以这里只需要忽略 Pong 帧，不需要自己发 Pong。
async fn reader_task(mut read: WsReader, tx: mpsc::Sender<ReaderMsg>, shutdown: CancellationToken) {
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                break;
            }
            frame = read.next() => {
                match frame {
                    None => {
                        // Stream 结束，对端关了。
                        let _ = tx.send(ReaderMsg::Closed(
                            "对端关闭了连接".to_owned(),
                        )).await;
                        break;
                    }
                    Some(Ok(msg)) => {
                        match msg {
                            Message::Text(t) => {
                                if tx.send(ReaderMsg::Text(t.to_string())).await.is_err() {
                                    break; // 主线程不收了
                                }
                            }
                            Message::Binary(b) => {
                                // DashScope 正常只发 text，但防御性地解码 binary。
                                let text = String::from_utf8_lossy(&b).into_owned();
                                if tx.send(ReaderMsg::Text(text)).await.is_err() {
                                    break;
                                }
                            }
                            Message::Close(frame) => {
                                let reason = match frame {
                                    Some(cf) => format!(
                                        "服务端关闭连接（{}）：{}",
                                        cf.code, cf.reason
                                    ),
                                    None => "服务端关闭连接（无附加信息）".to_owned(),
                                };
                                let _ = tx.send(ReaderMsg::Closed(reason)).await;
                                break;
                            }
                            // Ping: tungstenite 已自动回了 Pong，这里不需要做任何事。
                            // Pong: 忽略。
                            Message::Ping(_) | Message::Pong(_) => {}
                            // tungstenite 0.30 还有个 Frame variant，不应该出现在这里。
                            _ => {}
                        }
                    }
                    Some(Err(e)) => {
                        let reason = format!("连接读取出错：{e}");
                        let _ = tx.send(ReaderMsg::Error(reason)).await;
                        break;
                    }
                }
            }
        }
    }
}

fn reader_msg(msg: ReaderMsg) -> Incoming {
    match msg {
        ReaderMsg::Text(text) => Incoming::Text(text),
        ReaderMsg::Closed(reason) | ReaderMsg::Error(reason) => Incoming::Closed(reason),
    }
}

// --- 错误映射 ----------------------------------------------------------------

/// 把 tungstenite 的连接错误翻译成中文 PortError，并尽量区分失败类别。
fn map_connect_error(err: tungstenite::Error) -> PortError {
    match &err {
        tungstenite::Error::Http(response) => {
            let status = response.status();
            let body = response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b))
                .unwrap_or_default();
            PortError::new(format!("服务端拒绝了连接（HTTP {status}）：{body}",))
        }
        tungstenite::Error::HttpFormat(_) => PortError::new(format!("HTTP 协议格式错误：{err}")),
        tungstenite::Error::Io(io_err) => {
            let kind = io_err.kind();
            // 区分 DNS / TCP 连接失败。
            if format!("{err}").contains("dns")
                || format!("{err}").contains("resolve")
                || format!("{err}").contains("getaddrinfo")
            {
                PortError::new(format!("DNS 解析失败，检查网络连接：{err}"))
            } else {
                PortError::new(format!("网络连接失败（{kind:?}）：{err}"))
            }
        }
        tungstenite::Error::Tls(_) => {
            PortError::new(format!("TLS 握手失败，可能是证书或网络代理问题：{err}"))
        }
        _ => PortError::new(format!("WebSocket 连接失败：{err}")),
    }
}

// --- 测试 --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::SinkExt;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    use vox_core::cloud::{ConnectRequest, Incoming, Transport};

    /// 测试用的 runtime。服务器跑在里面，transport 在当前线程用 `new(handle)` 构造
    /// 并直接调用——因为测试是普通 `#[test]`，不在 async context 里，所以 block_on 合法。
    fn test_runtime() -> Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    /// 起一个本地 WebSocket echo 服务器。
    async fn start_echo_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                let (mut write, mut read) = ws.split();
                while let Some(Ok(msg)) = read.next().await {
                    match msg {
                        Message::Text(_) | Message::Binary(_) => {
                            if write.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        });
        addr
    }

    /// 起一个带自定义握手逻辑的服务器（能检查请求头）。
    /// 通过 oneshot 把收到的 Authorization 头传出来。
    async fn start_header_checking_server(
        auth_tx: tokio::sync::oneshot::Sender<Option<String>>,
    ) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let mut captured_auth: Option<String> = None;
                let callback = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                    captured_auth = req
                        .headers()
                        .get("Authorization")
                        .map(|v| v.to_str().unwrap_or("").to_string());
                    Ok(resp)
                };
                let ws = tokio_tungstenite::accept_hdr_async(stream, callback)
                    .await
                    .unwrap();
                let _ = auth_tx.send(captured_auth);
                let (mut write, mut read) = ws.split();
                while let Some(Ok(msg)) = read.next().await {
                    match msg {
                        Message::Text(_) | Message::Binary(_) => {
                            if write.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
        });
        addr
    }

    /// 起一个收到连接就主动关闭的服务器。
    async fn start_closing_server(code: u16, reason: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                let (mut write, _read) = ws.split();
                let close_frame = tungstenite::protocol::CloseFrame {
                    code: tungstenite::protocol::frame::coding::CloseCode::from(code),
                    reason: reason.into(),
                };
                let _ = write.send(Message::Close(Some(close_frame))).await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        addr
    }

    /// 起一个直接回 HTTP 401 的服务器（不做 WebSocket 升级）。
    async fn start_rejecting_server(status: u16) -> SocketAddr {
        use tokio::io::AsyncWriteExt;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = tokio::time::timeout(
                    Duration::from_millis(500),
                    tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
                )
                .await;
                let body = format!("rejected with {status}");
                let resp = format!(
                    "HTTP/1.1 {status} Rejected\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\r\n\
                     {body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        addr
    }

    fn make_request(addr: SocketAddr) -> ConnectRequest {
        ConnectRequest {
            url: format!("ws://127.0.0.1:{}", addr.port()),
            auth_header: "Bearer sk-test-key".to_owned(),
        }
    }

    #[test]
    fn connect_send_recv_roundtrip() {
        let rt = test_runtime();
        let addr = rt.block_on(start_echo_server());
        let mut transport = WsTransport::new(rt.handle().clone());

        transport.connect(&make_request(addr)).unwrap();
        transport.send(r#"{"hello":"world"}"#).unwrap();

        let msg = transport.recv(2000).unwrap();
        assert_eq!(msg, Some(Incoming::Text(r#"{"hello":"world"}"#.to_owned())));
    }

    #[test]
    fn authorization_header_arrives_at_server() {
        let rt = test_runtime();
        let (auth_tx, auth_rx) = tokio::sync::oneshot::channel();
        let addr = rt.block_on(start_header_checking_server(auth_tx));
        let mut transport = WsTransport::new(rt.handle().clone());

        transport.connect(&make_request(addr)).unwrap();
        transport.send("ping").unwrap();
        let _ = transport.recv(500).unwrap();
        transport.close();

        let captured = rt.block_on(auth_rx).unwrap();
        assert_eq!(captured, Some("Bearer sk-test-key".to_owned()));
    }

    #[test]
    fn recv_timeout_returns_none_and_connection_survives() {
        let rt = test_runtime();
        let addr = rt.block_on(start_echo_server());
        let mut transport = WsTransport::new(rt.handle().clone());
        transport.connect(&make_request(addr)).unwrap();

        // 没人给我们发消息，超时应该返回 None。
        let result = transport.recv(200).unwrap();
        assert_eq!(result, None, "超时应该返回 None 而不是错误");

        // 连接还活着——能正常收发。
        transport.send("still alive").unwrap();
        let msg = transport.recv(2000).unwrap();
        assert_eq!(msg, Some(Incoming::Text("still alive".to_owned())));
    }

    #[test]
    fn server_close_surfaces_as_incoming_closed() {
        let rt = test_runtime();
        let addr = rt.block_on(start_closing_server(1000, "再见"));
        let mut transport = WsTransport::new(rt.handle().clone());
        transport.connect(&make_request(addr)).unwrap();

        let msg = transport.recv(2000).unwrap();
        match msg {
            Some(Incoming::Closed(reason)) => {
                assert!(reason.contains("1000"), "应该包含 close code: {reason}");
                assert!(reason.contains("再见"), "应该包含 reason 文本: {reason}");
            }
            other => panic!("应该是 Closed，实际是 {other:?}"),
        }
    }

    #[test]
    fn close_twice_does_not_panic() {
        let rt = test_runtime();
        let addr = rt.block_on(start_echo_server());
        let mut transport = WsTransport::new(rt.handle().clone());
        transport.connect(&make_request(addr)).unwrap();
        transport.close();
        transport.close();
    }

    #[test]
    fn rejected_handshake_produces_port_error_with_status() {
        let rt = test_runtime();
        let addr = rt.block_on(start_rejecting_server(401));
        let mut transport = WsTransport::new(rt.handle().clone());

        let err = transport.connect(&make_request(addr)).unwrap_err();
        assert!(
            err.message.contains("401"),
            "错误消息里应该有 HTTP 状态码: {}",
            err.message
        );
    }

    #[test]
    fn send_before_connect_fails_gracefully() {
        let rt = test_runtime();
        let mut transport = WsTransport::new(rt.handle().clone());
        let err = transport.send("hello").unwrap_err();
        assert!(err.message.contains("未建立"));
    }

    #[test]
    fn recv_before_connect_fails_gracefully() {
        let rt = test_runtime();
        let mut transport = WsTransport::new(rt.handle().clone());
        let err = transport.recv(100).unwrap_err();
        assert!(err.message.contains("未建立"));
    }
}
