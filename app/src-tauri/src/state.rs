//! 全局状态：Tauri 命令、悬浮窗线程、热键线程共用的那一份。
//!
//! `Runtime` 自己是 `Arc<Inner>` 的壳，克隆等于共享，所以这里直接存值。
//! 除此之外只放三样东西：设备注册表（枚举设备要用）、落盘器、以及内核不外露
//! 但前端要的那点派生状态（完整 `GateStatus`）。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::OnceLock;

use parking_lot::Mutex;
use vox_core::event::Pipeline;
use vox_core::gate::GateStatus;
use vox_core::ports::DeviceRegistry;
use vox_core::{PipelineEngine, Runtime};

/// 悬浮窗把手。
pub type OverlayHandle = Arc<vox_overlay_win::Overlay>;

pub struct AppState {
    pub runtime: Runtime,
    pub engine: Arc<PipelineEngine>,
    pub registry: Arc<dyn DeviceRegistry>,
    pub persist: Arc<crate::persist::Persist>,
    /// 每条流水线最近一次的完整 `GateStatus`。
    ///
    /// 内核的 `PipelineSnapshot` 只留了 `gate_rms` / `gate_open`，前端要的是整个
    /// `GateStatus`（还要 `kind`/`state`/`ended`）。事件里能拿到全的，缓存下来给
    /// 快照用，比从两个标量硬凑一个假的诚实。
    gates: Mutex<BTreeMap<Pipeline, GateStatus>>,
    /// 悬浮窗，起完才有。
    pub overlay: OnceLock<OverlayHandle>,
}

impl AppState {
    pub fn new(
        runtime: Runtime,
        engine: Arc<PipelineEngine>,
        registry: Arc<dyn DeviceRegistry>,
        persist: Arc<crate::persist::Persist>,
    ) -> Self {
        Self {
            runtime,
            engine,
            registry,
            persist,
            gates: Mutex::new(BTreeMap::new()),
            overlay: OnceLock::new(),
        }
    }

    /// 记下一条流水线最新的门状态。
    pub fn remember_gate(&self, pipeline: Pipeline, status: GateStatus) {
        self.gates.lock().insert(pipeline, status);
    }

    /// 取缓存的门状态。流水线没在跑时一律当没有——跟前端 mock 的规则一致，
    /// 否则界面会一直显示上一次跑的时候的残留电平。
    pub fn gate_of(&self, pipeline: Pipeline, running: bool) -> Option<GateStatus> {
        if !running {
            return None;
        }
        self.gates.lock().get(&pipeline).copied()
    }

    /// 流水线停了，把门状态丢掉。
    pub fn forget_gate(&self, pipeline: Pipeline) {
        self.gates.lock().remove(&pipeline);
    }
}
