//! 设备清单：`DeviceRegistry` 的 Windows 实现。
//!
//! 这个类型是 `Send + Sync` 的，UI 线程随时会来查，所以每次调用自己建一个
//! `ComGuard`——不假设调用方线程初始化过 COM，也不缓存任何 COM 接口
//! （接口对象跨套间用是错的，而且设备热插拔后缓存就过期了）。

use vox_core::ports::{AudioApp, DeviceInfo, DeviceRegistry, PortResult};

use crate::cable::{self, CableStatus};
use crate::com::ComGuard;
use crate::devices;
use crate::sessions;

/// Windows 设备清单。
#[derive(Debug, Default, Clone, Copy)]
pub struct WinDeviceRegistry;

impl WinDeviceRegistry {
    pub fn new() -> Self {
        Self
    }
}

impl DeviceRegistry for WinDeviceRegistry {
    fn input_devices(&self) -> PortResult<Vec<DeviceInfo>> {
        let _com = ComGuard::mta()?;
        devices::list_devices(devices::CAPTURE)
    }

    fn output_devices(&self) -> PortResult<Vec<DeviceInfo>> {
        let _com = ComGuard::mta()?;
        devices::list_devices(devices::RENDER)
    }

    fn audio_apps(&self) -> PortResult<Vec<AudioApp>> {
        let _com = ComGuard::mta()?;
        sessions::audio_apps()
    }

    fn virtual_cable_installed(&self) -> bool {
        // 只看“装好了能用”这一种状态：待重启时端点还没出现，
        // 上层要是当成装好了就会去开一个不存在的设备。
        matches!(cable::detect(), CableStatus::Installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_devices_are_listable() {
        let reg = WinDeviceRegistry::new();
        // CI 机器可能没有任何录音设备，所以只要求不报错。
        let list = reg.input_devices().unwrap();
        for d in &list {
            assert!(!d.name.is_empty());
        }
        assert!(list.iter().filter(|d| d.is_default).count() <= 1);
    }

    #[test]
    fn output_devices_have_at_most_one_default() {
        let reg = WinDeviceRegistry::new();
        let list = reg.output_devices().unwrap();
        assert!(list.iter().filter(|d| d.is_default).count() <= 1);
    }

    #[test]
    fn audio_apps_never_include_self() {
        let reg = WinDeviceRegistry::new();
        let apps = reg.audio_apps().unwrap();
        let me = std::process::id();
        assert!(apps.iter().all(|a| a.pid != me));
    }

    #[test]
    fn virtual_cable_check_does_not_panic() {
        let reg = WinDeviceRegistry::new();
        let _ = reg.virtual_cable_installed();
    }

    #[test]
    fn registry_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WinDeviceRegistry>();
    }
}
