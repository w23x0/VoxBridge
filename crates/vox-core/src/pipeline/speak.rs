//! 对外说话：我说中文 → 对方听到外语。
//!
//! 麦克风 48 kHz → 单声道 → 降噪 → 阀门 → 16 kHz → 上传；
//! 服务端回的外语语音推给播放汇（外壳会把它指到 VB-CABLE，这样对方在
//! 语音软件里听到的是译文），译文文字进"暖白"那条字幕轨。
//!
//! 两条流水线里只有它认 `HotUpdate`（换目标语言、换音色不重连）。

use crate::cloud::{protocol::CloneFrequency, SessionParams};
use crate::ports::CaptureTarget;
use crate::runtime::SessionConfig;

use super::Plan;

pub(crate) fn plan(config: &SessionConfig) -> Plan {
    // 关掉「翻译」= 直通原声：不起云端会话，只把麦克风原声经闸门/降噪推给输出设备。
    // 此时没有译文/译音/字幕，`params` 里的语言音色都用不上。
    let passthrough = !config.translate;
    Plan {
        // `None` = 系统默认麦克风。
        target: CaptureTarget::Microphone(config.input_device.clone()),
        // 麦克风收的是真实空气声，空调、键盘、风扇都在里面，得降。
        denoise: config.denoise,
        passthrough,
        // 译文语音要出声；具体推哪个设备由设置定（一般是 VB-CABLE）。
        // 直通模式下同样是送去 output_device，只是内容成了原声、打开率用采集率。
        playback_device: if passthrough {
            Some(config.output_device.clone())
        } else {
            config.voice.as_ref().map(|_| config.output_device.clone())
        },
        // 测试时再复制一份到系统默认播放设备；没要语音时自然也无需回听。
        // 直通没有译文，回听没意义。
        monitor_translation: !passthrough && config.monitor_translation && config.voice.is_some(),
        hot_update: !passthrough,
        params: SessionParams {
            model_name: config.model_name.clone(),
            target_language: config.target_language.clone(),
            voice: config.voice.clone(),
            clone_frequency: config
                .voice_clone_frequency
                .and_then(CloneFrequency::from_count),
            // 对外说话只说，不做源文识别。
            source_language: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests::speak_config;

    #[test]
    fn speak_captures_the_microphone_and_denoises() {
        let config = speak_config();
        let plan = plan(&config);
        assert!(matches!(plan.target, CaptureTarget::Microphone(None)));
        assert!(plan.denoise, "麦克风收的是空气声，必须降噪");
        assert!(plan.hot_update, "只有对外说话认热更新");
    }

    #[test]
    fn speak_opens_playback_only_when_a_voice_is_picked() {
        let mut config = speak_config();
        assert!(plan(&config).playback_device.is_some());
        // 不要语音就别开播放汇，白占一个设备。
        config.voice = None;
        assert!(plan(&config).playback_device.is_none());
    }

    #[test]
    fn headphone_monitor_only_opens_for_synthesized_voice() {
        let mut config = speak_config();
        config.monitor_translation = true;
        assert!(plan(&config).monitor_translation);
        config.voice = None;
        assert!(!plan(&config).monitor_translation, "没有译音就不该开空回听");
    }

    #[test]
    fn speak_passes_the_clone_frequency_through() {
        let mut config = speak_config();
        config.voice_clone_frequency = Some(1);
        assert_eq!(
            plan(&config).params.clone_frequency,
            Some(CloneFrequency::Once)
        );
        config.voice_clone_frequency = Some(5);
        assert_eq!(
            plan(&config).params.clone_frequency,
            Some(CloneFrequency::Always)
        );
        // 0 次 = 不复刻。
        config.voice_clone_frequency = Some(0);
        assert_eq!(plan(&config).params.clone_frequency, None);
    }

    #[test]
    fn closing_translate_turns_speak_into_original_voice_passthrough() {
        let mut config = speak_config();
        config.translate = false;
        let plan = plan(&config);
        assert!(plan.passthrough, "关掉翻译就该是原声直通，不接云端");
        assert!(!plan.monitor_translation, "直通没有译文，不该开回听");
        assert!(
            plan.playback_device == Some(config.output_device.clone()),
            "直通要把原声送到设置的输出设备（VB-CABLE），而不是跟着音色走空"
        );
    }

    #[test]
    fn opened_translate_always_keeps_translate_mode() {
        let config = speak_config();
        assert!(!plan(&config).passthrough, "开着翻译就是正常对外说话");
    }
}
