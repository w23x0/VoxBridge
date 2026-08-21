//! `Transport` 端口的实现：直接用 `vox-net::WsTransport`。
//!
//! **复用 Tauri 自己的 tokio runtime**，不另起第二个（ARCHITECTURE.md §6）。
//! 每个会话一个 `WsTransport`，共享同一个 `Handle`——socket 各自独立，
//! 但都跑在同一批 tokio 工作线程上。

use tokio::runtime::Handle;
use vox_core::pipeline::TransportFactory;

pub fn transport_factory(handle: Handle) -> TransportFactory {
    Box::new(move || Box::new(vox_net::WsTransport::new(handle.clone())))
}
