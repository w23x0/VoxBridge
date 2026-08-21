//! 快照的对外形状。
//!
//! 内核 `runtime::Snapshot` 和前端 `types.snapshot.ts` 的字段名有少量偏差，
//! 在这里做一层映射。
//!
//! 规则：
//! - 前端要的每一个字段必须在、名字一字不差——缺字段前端会拿到 `undefined`。
//! - 多余字段无害（TS 不读罢了），所以 `virtual_cable_installed` 保留着不去掉。
//! - `Option` 字段**不加** `skip_serializing_if`——前端要的是显式 `null`。

use serde::Serialize;

use vox_core::event::{Notice, Pipeline, PipelineState};
use vox_core::gate::GateStatus;
use vox_core::latency::LatencySnapshot;
use vox_core::ports::{AudioApp, DeviceInfo};
use vox_core::runtime::Snapshot as CoreSnapshot;
use vox_core::settings::{ListenSettings, Settings, SpeakSettings, SubtitleSettings};
use vox_core::usage::UsageLedger;

use crate::state::AppState;

// ─── 设置 ────────────────────────────────────────────────────────────────────

/// 前端 `Settings` 的镜像。v2 起模型跟着两条流水线分别保存，嵌套结构体可以透传；
/// 内核顶层只保留一个不再序列化的 v1 迁移字段，所以这里不暴露它。
#[derive(Debug, Clone, Serialize)]
pub struct SettingsDto {
    pub version: u32,
    pub speak: SpeakSettings,
    pub listen: ListenSettings,
    pub subtitle: SubtitleSettings,
    pub autostart: bool,
    pub start_minimized: bool,
}

impl From<Settings> for SettingsDto {
    fn from(s: Settings) -> Self {
        Self {
            version: s.version,
            speak: s.speak,
            listen: s.listen,
            subtitle: s.subtitle,
            autostart: s.autostart,
            start_minimized: s.start_minimized,
        }
    }
}

/// 兼容旧 UI 偶尔发来的顶层 `model`：复制到两条流水线的 `model_name`。
/// 新 UI 直接发 `speak.model_name` / `listen.model_name`，本函数不会改它们。
pub fn patch_to_kernel(mut patch: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = patch.as_object_mut() {
        if let Some(model) = obj.remove("model") {
            for pipeline in ["speak", "listen"] {
                let entry = obj
                    .entry(pipeline.to_string())
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(fields) = entry.as_object_mut() {
                    fields
                        .entry("model_name".to_string())
                        .or_insert_with(|| model.clone());
                }
            }
        }
    }
    patch
}

// ─── 顶层 DTO ───────────────────────────────────────────────────────────────

/// 前端 `Snapshot` 的完整镜像。
#[derive(Debug, Clone, Serialize)]
pub struct SnapshotDto {
    pub settings: SettingsDto,
    /// 前端字段名 `has_api_key`；内核给的是 `api_key_configured`。
    pub has_api_key: bool,
    pub api_keys: ProviderKeyStatusDto,
    pub speak: PipelineSnapshotDto,
    pub listen: PipelineSnapshotDto,
    pub mic_active: bool,
    pub headphones_advised: bool,
    pub devices: DeviceSnapshotDto,
    pub usage: UsageLedger,
    pub notices: Vec<Notice>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderKeyStatusDto {
    pub aliyun: bool,
    pub gemini: bool,
}

// ─── 流水线 ──────────────────────────────────────────────────────────────────

/// 前端 `PipelineSnapshot`：多了 `running` 和完整 `gate`。
#[derive(Debug, Clone, Serialize)]
pub struct PipelineSnapshotDto {
    pub state: PipelineState,
    pub state_label: &'static str,
    /// 前端需要的布尔，内核没直接给——从 `PipelineState::is_running()` 派生。
    pub running: bool,
    /// 完整的五字段门状态。不跑的时候为 `null`（跟 mock 一致）。
    pub gate: Option<GateStatus>,
    /// 前端没用到，但内核给了、留着无害。
    pub last_error: Option<String>,
    /// 逐轮延迟统计（内核 `LatencySnapshot` 的同形映射）。
    pub latency: LatencySnapshot,
}

// ─── 设备 ────────────────────────────────────────────────────────────────────

/// 前端 `DeviceSnapshot`：`audio_apps` 要重命名为 `apps`。
#[derive(Debug, Clone, Serialize)]
pub struct DeviceSnapshotDto {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    /// 前端字段名 `apps`；内核给的是 `audio_apps`。
    pub apps: Vec<AudioApp>,
    /// 前端没有这个字段，但保留着不会出错（多余字段被 TS 忽略）。
    pub virtual_cable_installed: bool,
    /// `installed` / `install_pending_reboot` / `uninstall_incomplete` / `not_installed`。
    pub virtual_cable_status: &'static str,
    /// `visible` / `hidden` / `absent`，用于管理新版附带的 16 声道端点。
    pub virtual_cable_16ch_status: &'static str,
}

// ─── 转换函数 ────────────────────────────────────────────────────────────────

/// 从 `AppState` 取内核快照，再加上装配层缓存的门状态，拼成前端要的 DTO。
pub fn snapshot(state: &AppState) -> SnapshotDto {
    let core: CoreSnapshot = state.runtime.snapshot();

    let speak_running = core.speak.state.is_running();
    let listen_running = core.listen.state.is_running();

    SnapshotDto {
        settings: core.settings.into(),
        has_api_key: core.api_key_configured,
        api_keys: ProviderKeyStatusDto {
            aliyun: core
                .api_keys_configured
                .get(&vox_core::settings::ModelProvider::Aliyun)
                .copied()
                .unwrap_or(false),
            gemini: core
                .api_keys_configured
                .get(&vox_core::settings::ModelProvider::Gemini)
                .copied()
                .unwrap_or(false),
        },
        speak: pipeline_dto(&core.speak, state.gate_of(Pipeline::Speak, speak_running)),
        listen: pipeline_dto(
            &core.listen,
            state.gate_of(Pipeline::Listen, listen_running),
        ),
        mic_active: core.mic_active,
        headphones_advised: core.headphones_advised,
        devices: devices_dto(core.devices),
        usage: core.usage,
        notices: core.notices,
    }
}

/// 内核 `PipelineSnapshot` → 前端形状。
///
/// `gate` 来自装配层缓存，不跑时已经被 `AppState::gate_of` 过滤成 `None`。
fn pipeline_dto(
    core: &vox_core::runtime::PipelineSnapshot,
    gate: Option<GateStatus>,
) -> PipelineSnapshotDto {
    PipelineSnapshotDto {
        state: core.state,
        state_label: core.state_label,
        running: core.state.is_running(),
        gate,
        last_error: core.last_error.clone(),
        latency: core.latency.clone(),
    }
}

/// 内核 `DeviceSnapshot` → 前端形状（`audio_apps` 重命名为 `apps`）。
fn devices_dto(core: vox_core::runtime::DeviceSnapshot) -> DeviceSnapshotDto {
    let cable_status = match vox_audio_win::cable::detect() {
        vox_audio_win::CableStatus::Installed => "installed",
        vox_audio_win::CableStatus::InstalledPendingReboot => "install_pending_reboot",
        vox_audio_win::CableStatus::UninstallIncomplete => "uninstall_incomplete",
        vox_audio_win::CableStatus::NotInstalled => "not_installed",
    };
    let channel_status = match vox_audio_win::multichannel_endpoint_status() {
        vox_audio_win::MultichannelEndpointStatus::Enabled => "visible",
        vox_audio_win::MultichannelEndpointStatus::Disabled => "hidden",
        vox_audio_win::MultichannelEndpointStatus::NotPresent => "absent",
    };
    DeviceSnapshotDto {
        inputs: core.inputs,
        outputs: core.outputs,
        apps: core.audio_apps,
        virtual_cable_installed: core.virtual_cable_installed,
        virtual_cable_status: cable_status,
        virtual_cable_16ch_status: channel_status,
    }
}

// ─── 测试 ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use vox_core::event::PipelineState;
    use vox_core::gate::{GateKind, GateState};

    /// 造一个最小的 `SnapshotDto`，用来验证序列化后的 JSON 形状。
    fn make_dto(running: bool, gate: Option<GateStatus>) -> SnapshotDto {
        let state = if running {
            PipelineState::Ready
        } else {
            PipelineState::Idle
        };
        let label = state.label();
        let pipe = PipelineSnapshotDto {
            state,
            state_label: label,
            running,
            gate,
            last_error: None,
            latency: LatencySnapshot::default(),
        };
        SnapshotDto {
            settings: Settings::default().into(),
            has_api_key: true,
            api_keys: ProviderKeyStatusDto {
                aliyun: true,
                gemini: false,
            },
            speak: pipe.clone(),
            listen: pipe,
            mic_active: false,
            headphones_advised: false,
            devices: DeviceSnapshotDto {
                inputs: vec![],
                outputs: vec![],
                apps: vec![],
                virtual_cable_installed: false,
                virtual_cable_status: "not_installed",
                virtual_cable_16ch_status: "absent",
            },
            usage: UsageLedger::default(),
            notices: vec![],
        }
    }

    fn sample_gate() -> GateStatus {
        GateStatus {
            kind: GateKind::Level,
            state: GateState::Speech,
            rms: 0.42,
            active: true,
            ended: false,
        }
    }

    /// 前端要的所有顶层字段都在。
    #[test]
    fn snapshot_has_all_frontend_keys() {
        let dto = make_dto(true, Some(sample_gate()));
        let json = serde_json::to_value(&dto).unwrap();

        let expected_keys = [
            "settings",
            "has_api_key",
            "api_keys",
            "speak",
            "listen",
            "mic_active",
            "headphones_advised",
            "devices",
            "usage",
            "notices",
        ];
        for key in expected_keys {
            assert!(json.get(key).is_some(), "顶层缺字段: {key}");
        }
    }

    /// 不含只用于 v1 迁移的顶层模型字段——防止旧结构重新漏给前端。
    #[test]
    fn snapshot_no_stale_core_names() {
        let dto = make_dto(true, Some(sample_gate()));
        let json = serde_json::to_value(&dto).unwrap();
        let settings = json.get("settings").unwrap();

        assert!(settings.get("model_name").is_none(), "顶层共用模型已废弃");
        assert!(settings.get("model").is_none(), "旧 UI 的顶层 model 已废弃");
        assert!(settings["speak"].get("model_name").is_some());
        assert!(settings["listen"].get("model_name").is_some());
    }

    /// 两条流水线各自带服务商和模型。
    #[test]
    fn snapshot_settings_has_per_pipeline_provider_and_model() {
        let dto = make_dto(true, None);
        let json = serde_json::to_value(&dto).unwrap();
        let settings = json.get("settings").unwrap();
        for pipeline in ["speak", "listen"] {
            assert_eq!(settings[pipeline]["provider"], "aliyun");
            assert_eq!(
                settings[pipeline]["model_name"],
                Value::String(vox_core::catalog::DEFAULT_MODEL_NAME.to_string())
            );
        }
    }

    #[test]
    fn snapshot_settings_has_translation_switches() {
        let dto = make_dto(true, None);
        let settings = serde_json::to_value(&dto).unwrap()["settings"].clone();

        assert_eq!(settings["speak"]["show_translation"], true);
        assert_eq!(settings["speak"]["speak_translation"], true);
        assert_eq!(settings["listen"]["show_translation"], true);
        assert_eq!(settings["listen"]["speak_translation"], true);
    }

    /// 旧 UI 的顶层 model 会同时迁到两条流水线，新 UI 不受影响。
    #[test]
    fn patch_to_kernel_expands_legacy_model() {
        let patch = serde_json::json!({
            "model": "retired-model",
            "speak": { "enabled": true }
        });
        let out = patch_to_kernel(patch);
        assert_eq!(
            out["speak"]["model_name"],
            Value::String("retired-model".to_string())
        );
        assert_eq!(out["listen"]["model_name"], out["speak"]["model_name"]);
        assert!(out.get("model").is_none());
        assert_eq!(out["speak"]["enabled"], Value::Bool(true));
    }

    /// patch 里没带 `model` 键时不该凑一个出来。
    #[test]
    fn patch_to_kernel_no_op_without_model() {
        let patch = serde_json::json!({ "speak": { "enabled": true } });
        let out = patch_to_kernel(patch.clone());
        assert_eq!(out, patch);
    }

    /// 流水线子对象包含前端需要的四个字段。
    #[test]
    fn pipeline_dto_shape() {
        let dto = make_dto(true, Some(sample_gate()));
        let json = serde_json::to_value(&dto).unwrap();

        let speak = json.get("speak").unwrap();
        for key in ["state", "state_label", "running", "gate", "latency"] {
            assert!(speak.get(key).is_some(), "speak 缺字段: {key}");
        }
        assert_eq!(speak.get("running").unwrap(), &Value::Bool(true));
    }

    /// 不跑的时候 `gate` 序列化成 JSON `null`——不是字段消失。
    #[test]
    fn gate_is_null_when_not_running() {
        let dto = make_dto(false, None);
        let json = serde_json::to_value(&dto).unwrap();

        let speak = json.get("speak").unwrap();
        // 字段必须存在
        assert!(speak.get("gate").is_some(), "gate 字段不该消失");
        // 值必须是 null
        assert!(speak.get("gate").unwrap().is_null(), "gate 应为 null");
        assert_eq!(speak.get("running").unwrap(), &Value::Bool(false));
    }

    /// 跑起来的时候 `gate` 是五字段对象。
    #[test]
    fn gate_has_five_fields_when_running() {
        let dto = make_dto(true, Some(sample_gate()));
        let json = serde_json::to_value(&dto).unwrap();

        let gate = json.get("speak").unwrap().get("gate").unwrap();
        assert!(gate.is_object(), "gate 应为对象");
        for key in ["kind", "state", "rms", "active", "ended"] {
            assert!(gate.get(key).is_some(), "gate 缺字段: {key}");
        }
    }

    /// devices 子对象里叫 `apps` 不叫 `audio_apps`。
    #[test]
    fn devices_uses_apps_not_audio_apps() {
        let dto = make_dto(false, None);
        let json = serde_json::to_value(&dto).unwrap();

        let devices = json.get("devices").unwrap();
        assert!(devices.get("apps").is_some(), "devices 缺 apps 字段");
        assert!(
            devices.get("audio_apps").is_none(),
            "devices 不该有 audio_apps"
        );
        assert!(devices.get("inputs").is_some());
        assert!(devices.get("outputs").is_some());
    }
}
