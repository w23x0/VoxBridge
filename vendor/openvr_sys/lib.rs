#![allow(non_camel_case_types, non_upper_case_globals, non_snake_case)]

include!("bindings.rs");

// Compatibility names used by openvr 0.9 but omitted by newer headers.
pub const ETrackedDeviceProperty_Prop_PreviousUniverseId_Uint64_deprecated: ETrackedDeviceProperty = ETrackedDeviceProperty_Prop_PreviousUniverseId_Uint64;
pub const EVREventType_VREvent_ChaperoneRoomSetupCommitted: EVREventType = EVREventType_VREvent_ChaperoneRoomSetupFinished;
pub const EVRSettingsError_VRSettingsError_AccessDenied: EVRSettingsError = 6;

impl Default for TrackedDevicePose_t {
    fn default() -> Self { unsafe { std::mem::zeroed() } }
}
impl Default for Compositor_FrameTiming {
    fn default() -> Self { unsafe { std::mem::zeroed() } }
}

#[cfg(target_os = "macos")]
#[link(name = "Foundation", kind = "framework")]
extern "C" {}
