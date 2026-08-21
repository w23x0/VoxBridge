//! 设备自动选择的打分规则。
//!
//! 这些分数是旧版 devices.py 里调出来的资产，原样搬过来，不要重新推导。
//! 它们编码的是 Windows 设备命名的一堆经验：Voicemeeter 的通道命名、
//! VB-CABLE 的端点名、Realtek 阵列麦的名字长相，等等。
//!
//! 唯一的改动：旧版要区分 WASAPI / MME / DirectSound 三套重名设备，
//! 现在全走 WASAPI 端点，所以那几层 hostapi 判断合并掉了——分档的相对顺序保持不变。

use vox_core::ports::DeviceInfo;

/// 名字看着像虚拟声卡就算虚拟设备。
pub fn is_virtual_audio_device(name: &str) -> bool {
    let text = name.to_lowercase();
    const KEYWORDS: [&str; 5] = ["vb-audio", "vb-cable", "cable", "voicemeeter", "virtual"];
    KEYWORDS.iter().any(|k| text.contains(k))
}

fn contains_mic(text: &str) -> bool {
    text.contains("microphone") || text.contains("麦克风")
}

/// 麦克风打分，越小越优先。
fn microphone_score(device: &DeviceInfo) -> u32 {
    let text = device.name.to_lowercase();
    if is_virtual_audio_device(&text) {
        // 虚拟设备当麦克风几乎总是错的（会把自己的输出录回去），压到最后。
        return 90;
    }
    if text.contains("microsoft 声音映射器")
        || text.contains("主声音捕获驱动程序")
        || text.contains("primary sound capture")
    {
        return 92;
    }
    if text.contains("stereo mix") || text.contains("立体声混音") {
        return 91;
    }
    if contains_mic(&text)
        && (text.contains("realtek")
            || text.contains("usb")
            || text.contains("array")
            || text.contains("阵列"))
    {
        return 0;
    }
    if contains_mic(&text) && text.contains("()") {
        // 名字里带空括号的一般是驱动没填全的残缺端点，能用但排后面。
        return 8;
    }
    if contains_mic(&text) {
        return 2;
    }
    if text.contains("headset") || text.contains("hands-free") || text.contains("hands free") {
        return 3;
    }
    if text.contains("耳机") {
        return 4;
    }
    if device.is_default {
        return 5;
    }
    20
}

/// 从输入设备里挑最像真麦克风的那个，返回下标。
pub fn pick_microphone(inputs: &[DeviceInfo]) -> Option<usize> {
    pick_min(inputs, microphone_score, 100)
}

/// 虚拟输出（把译文送给 VRChat 的那一端）打分。
fn virtual_output_score(device: &DeviceInfo) -> u32 {
    let text = device.name.to_lowercase();
    if text.contains("voicemeeter input") {
        return 0;
    }
    if text.contains("voicemeeter aux input") {
        return 1;
    }
    if text.contains("voicemeeter vaio3 input") {
        return 2;
    }
    if crate::cable::is_cable_render(&device.name) {
        // 同一驱动有普通端点和 16 声道端点时，优先普通端点；VoxBridge 只传人声。
        return if crate::cable::is_cable_multichannel_render(&device.name) {
            4
        } else {
            3
        };
    }
    if text.contains("vb-cable") {
        return 6;
    }
    if text.contains("voicemeeter") && text.contains("input") {
        return 7;
    }
    99
}

/// 挑虚拟输出端点。没有像样的候选就返回 `None`（此时该提示用户装 VB-CABLE）。
pub fn pick_virtual_output(outputs: &[DeviceInfo]) -> Option<usize> {
    let candidates: Vec<usize> = (0..outputs.len())
        .filter(|&i| is_virtual_audio_device(&outputs[i].name))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let best = candidates
        .into_iter()
        .min_by_key(|&i| (virtual_output_score(&outputs[i]), i))?;
    (virtual_output_score(&outputs[best]) < 99).then_some(best)
}

/// 听人说话用的虚拟采集端点打分。
fn listen_input_score(device: &DeviceInfo) -> u32 {
    let text = device.name.to_lowercase();
    if text.contains("voicemeeter output") || text.contains("voicemeeter out b1") {
        return 0;
    }
    if text.contains("voicemeeter aux output") || text.contains("voicemeeter out b2") {
        return 1;
    }
    if text.contains("voicemeeter vaio3 output") || text.contains("voicemeeter out b3") {
        return 2;
    }
    if text.contains("cable output") {
        return 3;
    }
    if is_virtual_audio_device(&text) && (text.contains("output") || text.contains(" out ")) {
        return 8;
    }
    99
}

/// 挑“听别人”用的采集端点。现在主路径是进程环回，这个是退路。
pub fn pick_listen_input(inputs: &[DeviceInfo]) -> Option<usize> {
    pick_min(inputs, listen_input_score, 99)
}

fn pick_min(
    devices: &[DeviceInfo],
    score: impl Fn(&DeviceInfo) -> u32,
    reject_at_or_above: u32,
) -> Option<usize> {
    let best = (0..devices.len()).min_by_key(|&i| (score(&devices[i]), i))?;
    (score(&devices[best]) < reject_at_or_above).then_some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            is_default: false,
        }
    }

    fn default_dev(name: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            is_default: true,
        }
    }

    #[test]
    fn virtual_keywords_detected() {
        assert!(is_virtual_audio_device(
            "CABLE Input (VB-Audio Virtual Cable)"
        ));
        assert!(is_virtual_audio_device("Voicemeeter Out B1"));
        assert!(is_virtual_audio_device("Some Virtual Thing"));
        assert!(!is_virtual_audio_device("麦克风 (Realtek(R) Audio)"));
    }

    #[test]
    fn real_mic_beats_virtual_and_stereo_mix() {
        let inputs = vec![
            dev("CABLE Output (VB-Audio Virtual Cable)"),
            dev("立体声混音 (Realtek(R) Audio)"),
            dev("麦克风 (Realtek(R) Audio)"),
        ];
        assert_eq!(pick_microphone(&inputs), Some(2));
    }

    #[test]
    fn usb_mic_beats_plain_mic() {
        let inputs = vec![
            dev("麦克风 (蓝牙耳机)"),
            dev("Microphone (USB Audio Device)"),
        ];
        assert_eq!(pick_microphone(&inputs), Some(1));
    }

    #[test]
    fn headset_beats_default_when_no_mic_named() {
        let inputs = vec![default_dev("Line In (Realtek)"), dev("Headset (HyperX)")];
        assert_eq!(pick_microphone(&inputs), Some(1));
    }

    #[test]
    fn empty_parens_mic_is_deprioritized() {
        let inputs = vec![dev("麦克风 ()"), dev("耳机 (Bluetooth)")];
        assert_eq!(pick_microphone(&inputs), Some(1));
    }

    #[test]
    fn only_virtual_inputs_still_returns_something() {
        // 全是虚拟设备时也得给一个，不然用户连开麦都开不了。
        let inputs = vec![dev("CABLE Output (VB-Audio Virtual Cable)")];
        assert_eq!(pick_microphone(&inputs), Some(0));
    }

    #[test]
    fn no_inputs_gives_none() {
        assert_eq!(pick_microphone(&[]), None);
        assert_eq!(pick_virtual_output(&[]), None);
        assert_eq!(pick_listen_input(&[]), None);
    }

    #[test]
    fn cable_input_picked_for_virtual_output() {
        let outputs = vec![
            default_dev("扬声器 (Realtek(R) Audio)"),
            dev("CABLE Input (VB-Audio Virtual Cable)"),
        ];
        assert_eq!(pick_virtual_output(&outputs), Some(1));
    }

    #[test]
    fn voicemeeter_input_outranks_cable_input() {
        let outputs = vec![
            dev("CABLE Input (VB-Audio Virtual Cable)"),
            dev("Voicemeeter Input (VB-Audio Voicemeeter VAIO)"),
        ];
        assert_eq!(pick_virtual_output(&outputs), Some(1));
    }

    #[test]
    fn no_virtual_output_when_only_real_speakers() {
        let outputs = vec![
            default_dev("扬声器 (Realtek(R) Audio)"),
            dev("耳机 (NVIDIA)"),
        ];
        assert_eq!(pick_virtual_output(&outputs), None);
    }

    #[test]
    fn new_driver_channel_name_picked_for_virtual_output() {
        // 新版驱动把播放端点叫 `CABLE In 16 Ch`，不是老名字 `CABLE Input`。
        // 认不出它的话，装好的虚拟声卡会永远"选不中"。
        let outputs = vec![
            default_dev("扬声器 (Realtek(R) Audio)"),
            dev("CABLE In 16 Ch (VB-Audio Virtual Cable)"),
        ];
        assert_eq!(pick_virtual_output(&outputs), Some(1));
    }

    #[test]
    fn normal_cable_endpoint_beats_16_channel_endpoint() {
        let outputs = vec![
            dev("CABLE In 16 Ch (VB-Audio Virtual Cable)"),
            dev("扬声器 (VB-Audio Virtual Cable)"),
        ];
        assert_eq!(pick_virtual_output(&outputs), Some(1));
    }

    #[test]
    fn channel_name_does_not_pollute_real_devices() {
        // `in` 是极常见的单词，放宽匹配后绝不能把真设备误当虚拟输出。
        let outputs = vec![
            default_dev("Line In (Realtek(R) Audio)"),
            dev("麦克风 (USB)"),
        ];
        assert_eq!(pick_virtual_output(&outputs), None);
    }

    #[test]
    fn cable_output_picked_for_listen_input() {
        let inputs = vec![
            dev("麦克风 (Realtek(R) Audio)"),
            dev("CABLE Output (VB-Audio Virtual Cable)"),
        ];
        assert_eq!(pick_listen_input(&inputs), Some(1));
    }

    #[test]
    fn no_listen_input_when_no_virtual_capture() {
        let inputs = vec![dev("麦克风 (Realtek(R) Audio)")];
        assert_eq!(pick_listen_input(&inputs), None);
    }

    #[test]
    fn ties_break_by_enumeration_order() {
        let inputs = vec![dev("Microphone (USB A)"), dev("Microphone (USB B)")];
        assert_eq!(pick_microphone(&inputs), Some(0));
    }
}
