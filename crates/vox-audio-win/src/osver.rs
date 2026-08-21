//! 系统版本判断。只服务一件事：进程环回能不能用。
//!
//! 进程环回（VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK）要求 Windows 内部版本
//! ≥ 20348。注意零售版 Win10 22H2 是 19045，**没有**这个接口——不是靠
//! “Win10 还是 Win11”能判断的，必须看 build 号。
//!
//! 用 `RtlGetVersion` 而不是 `GetVersionEx`：后者会被兼容性垫层骗，
//! 在没有清单声明的进程里可能只报 6.2。`RtlGetVersion` 报的是真实版本。

use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;

/// 进程环回要求的最低内部版本号（Windows Server 2022 / Win11 起）。
pub const MIN_PROCESS_LOOPBACK_BUILD: u32 = 20348;

/// 当前系统的内部版本号。取不到时返回 0（会被判定为不支持）。
pub fn os_build_number() -> u32 {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: 结构体大小字段已按文档填好，指针指向本地栈上的合法结构体；
    // RtlGetVersion 只写这一个结构体，返回 NTSTATUS 不抛异常。
    let status = unsafe { RtlGetVersion(&mut info) };
    if status.is_ok() {
        info.dwBuildNumber
    } else {
        0
    }
}

/// 纯判断，方便测试。
pub fn process_loopback_supported(build: u32) -> bool {
    build >= MIN_PROCESS_LOOPBACK_BUILD
}

/// 当前机器是否支持进程环回。
pub fn process_loopback_available() -> bool {
    process_loopback_supported(os_build_number())
}

/// 不支持时给用户看的话。调用方拿到这句应该退到整机环回。
pub(crate) fn unsupported_message(build: u32) -> String {
    format!(
        "当前系统内部版本 {build} 不支持按进程抓声音（需要 {MIN_PROCESS_LOOPBACK_BUILD} 及以上，\
         Win11 或 Server 2022 才有；Win10 22H2 是 19045，没有这个接口）。\
         请改用整机环回（抓默认输出设备）。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_gate_boundaries() {
        assert!(!process_loopback_supported(0));
        assert!(!process_loopback_supported(19045)); // 零售 Win10 22H2
        assert!(!process_loopback_supported(20347));
        assert!(process_loopback_supported(20348)); // Server 2022
        assert!(process_loopback_supported(22621)); // Win11 22H2
        assert!(process_loopback_supported(26200));
    }

    #[test]
    fn unsupported_message_names_the_number() {
        let msg = unsupported_message(19045);
        assert!(msg.contains("19045"));
        assert!(msg.contains("20348"));
    }

    #[test]
    fn real_machine_reports_a_plausible_build() {
        // 这台机器是 Win11 26200；任何 Win10 之后的系统都该 > 10000。
        let build = os_build_number();
        assert!(build > 10_000, "读到的内部版本号是 {build}");
    }
}
