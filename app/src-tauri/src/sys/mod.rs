//! 跟操作系统直接打交道的小件。
//!
//! 只有 `log` 是跨平台的（写 stderr / 日志文件）；另外三个是 Windows 专属，
//! Linux 的对应物在 `platform/linux/`（时钟、密钥库）与 `platform::alert`（致命提示）。

pub mod log;

#[cfg(windows)]
pub mod clock;
#[cfg(windows)]
pub mod fatal;
#[cfg(windows)]
pub mod secrets;
