//! tracing 初始化。
//!
//! 只往 stderr 写。发布构建是 `windows_subsystem = "windows"`，没有控制台，
//! 这些输出会被丢掉——不落文件是刻意的：日志里难免带上识别出来的原话，
//! 那是用户说的话，不该在磁盘上留一份。要排查问题就跑调试构建。

use tracing_subscriber::EnvFilter;

pub fn init() {
    let filter = EnvFilter::try_from_env("VOXBRIDGE_LOG")
        .unwrap_or_else(|_| EnvFilter::new("voxbridge_lib=debug,vox_=debug,warn"));
    // 重复初始化不算错（测试里可能已经装过一个）。
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_ansi(false)
        .try_init();
}
