//! `vox_core::cloud::Transport` 的 WebSocket 实现。
//!
//! 内核的 `Transport` trait 是同步阻塞的——流水线跑在普通 OS 线程上，不能
//! 要求调用方有 async context。这里用一个专属的 tokio 任务持有 socket，
//! 主线程通过 channel 与之通信。选 channel 方案而不是直接 `block_on` 的理由：
//!
//! 1. `recv(timeout_ms)` 需要精确超时且超时不能断连——`block_on` + `tokio::time::timeout`
//!    可以做到，但 cancel 一个正在 `read()` 的 future 会让 tungstenite 内部状态
//!    不可预知（文档没承诺 cancel-safe）；channel 方案里 socket 的读循环永远跑完
//!    一整帧才投递，不存在半帧问题。
//! 2. `close()` 要幂等且 ≤1s 完成——向任务发关闭信号即可，不必等远端回 close frame。
//! 3. Ping/Pong 在读循环里自动处理，不暴露给主线程。
//!
//! 构造方式：
//! - `WsTransport::new(handle)` —— 复用已有的 tokio runtime（Tauri 场景）。
//! - `WsTransport::standalone()` —— 自己起一个 2 线程的 runtime（测试/CLI 场景）。

mod ws;

pub use ws::WsTransport;
