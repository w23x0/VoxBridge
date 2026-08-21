//! 设备枚举线程。
//!
//! 枚举要走 COM，一次几十毫秒，绝不能在 Tauri 命令里同步做（会卡住 UI 线程）。
//! 所以：起个自己的线程，先立刻扫一次，之后低频轮询——插拔耳机、开关某个程序
//! 都会改变可选项，用户不该为了看到新设备去点刷新。
//!
//! 轮询而不是订阅 `IMMNotificationClient`：那个要 COM 回调对象和消息泵，
//! 复杂度换来的只是几秒的延迟差，不值。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use vox_core::ports::DeviceRegistry;
use vox_core::runtime::DeviceSnapshot;

use crate::state::AppState;

/// 两次自动扫描之间隔多久。够快能感知插拔，够慢不至于一直占着 COM。
const POLL_INTERVAL: Duration = Duration::from_secs(4);

static STOP: AtomicBool = AtomicBool::new(false);

/// 轮询线程句柄，`stop()` 里 join。
static THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

pub fn start(state: &Arc<AppState>) {
    let state = Arc::clone(state);
    let notify_rt = state.runtime.clone();
    let handle = std::thread::Builder::new()
        .name("vox-devices".into())
        .spawn(move || {
            // catch_unwind：这里 panic 的默认表现是"设备列表停止刷新"——插拔耳机
            // 不再出现在下拉框里，而界面上一切正常，用户只会觉得设备识别很烂。
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                loop {
                    let snapshot = scan(state.registry.as_ref());
                    state.runtime.set_devices(snapshot);
                    // 分段睡，好让退出时最多等 250 ms 而不是一整个周期。
                    let mut slept = Duration::ZERO;
                    while slept < POLL_INTERVAL {
                        if STOP.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                        slept += Duration::from_millis(250);
                    }
                }
            }));
            if result.is_err() {
                notify_rt.notify(vox_core::event::Notice::warning(
                    "设备枚举线程异常退出，设备列表不再自动刷新。可以手动点「重新扫描设备」。"
                        .to_string(),
                ));
            }
        })
        .ok(); // 起不来不致命：设备列表就一直是空的，UI 少几个选项，但应用能用。
    if handle.is_none() {
        tracing::warn!("设备枚举线程起不来，设备列表将为空");
    }
    *THREAD.lock() = handle;
}

/// 让轮询线程收工并等它退出（最多 250 ms）。进程退出时调。
///
/// 必须 join：这个线程会 `set_devices` → emit 事件 → listener 可能把账本标脏。
/// 只竖旗就 flush 的话，最后 250 ms 内的改动会静默丢掉。
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
    if let Some(h) = THREAD.lock().take() {
        let _ = h.join();
    }
}

/// 同步扫一遍。命令 `refresh_devices` 也用这个，但要在别的线程上跑。
pub fn scan(registry: &dyn DeviceRegistry) -> DeviceSnapshot {
    // 任一项失败就给空列表——UI 上少几个选项，比整个面板打不开好。
    DeviceSnapshot {
        inputs: registry.input_devices().unwrap_or_default(),
        outputs: registry.output_devices().unwrap_or_default(),
        audio_apps: registry.audio_apps().unwrap_or_default(),
        virtual_cable_installed: registry.virtual_cable_installed(),
    }
}
