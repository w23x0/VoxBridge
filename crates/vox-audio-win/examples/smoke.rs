//! 手动验一遍这个 crate 到底通不通。跑法：
//!
//! ```text
//! cargo run -p vox-audio-win --example smoke              # 只查，不出声
//! cargo run -p vox-audio-win --example smoke -- mic       # 录 3 秒麦克风
//! cargo run -p vox-audio-win --example smoke -- mic "麦克风 (Realtek)"
//! cargo run -p vox-audio-win --example smoke -- tone      # 默认输出放 1 秒 440 Hz
//! cargo run -p vox-audio-win --example smoke -- tone "CABLE Input (VB-Audio Virtual Cable)"
//! cargo run -p vox-audio-win --example smoke -- app msedge.exe   # 抓某个程序的声音 5 秒
//! ```
//!
//! 只读系统状态 + 用默认/指定设备收发音。**不下载、不安装任何东西**，
//! VB-CABLE 那栏只报“装了没”。
//!
//! 全在 Windows 上跑（`vox-audio-win` 在别的平台是空 lib），所以这里在非 Windows
//! 上只是个空 main。

#[cfg(windows)]
#[path = "smoke/windows.rs"]
mod windows;

#[cfg(windows)]
fn main() {
    windows::main();
}

#[cfg(not(windows))]
fn main() {}
