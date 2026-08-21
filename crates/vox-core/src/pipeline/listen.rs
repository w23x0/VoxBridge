//! 听人说话：对方说外语 → 我看/听到中文。
//!
//! 抓的不是麦克风而是**某个程序的环回**（Discord、浏览器……），所以：
//! - 信号是数字源，本来就干净，**不降噪**（白降一遍还费 CPU，还可能吃掉人声）。
//! - **不设电平门**（`GateConfig::level(0.0)` 无条件放行）：环回源有假底噪，
//!   电平门会被底噪骗"正在说话"而误开，白白烧 token；省 token 靠服务端 VAD。
//! - 中文语音走系统默认输出（耳机），不能推 VB-CABLE，不然对方会听到自己的译文。
//! - 不认 `HotUpdate`：听的方向永远译成中文。

use crate::cloud::SessionParams;
use crate::ports::{CaptureTarget, PortError};
use crate::runtime::SessionConfig;

use super::Plan;

pub(crate) fn plan(config: &SessionConfig) -> Result<Plan, PortError> {
    // 没选程序就没法抓环回。账本在 `start()` 里已经挡了一道，这里是兜底。
    let target = config
        .loopback_target
        .as_ref()
        .ok_or_else(|| PortError::new("还没选择监听程序。"))?;

    Ok(Plan {
        target: CaptureTarget::ProcessLoopback {
            executable: target.executable.clone(),
            // 浏览器那种多进程的，声音常在子进程里，得连带抓。
            include_tree: target.include_process_tree,
        },
        denoise: false,
        playback_device: config.voice.as_ref().map(|_| config.output_device.clone()),
        monitor_translation: false,
        hot_update: false,
        params: SessionParams {
            model_name: config.model_name.clone(),
            target_language: config.target_language.clone(),
            voice: config.voice.clone(),
            // 听别人说话没有"复刻我的音色"这回事。
            clone_frequency: None,
            // 源语言；None = 服务端自动识别。
            source_language: config.source_language.clone(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::tests::listen_config;

    #[test]
    fn listen_grabs_the_process_loopback_and_skips_denoise() {
        let plan = plan(&listen_config()).expect("选了程序就该能派活");
        match &plan.target {
            CaptureTarget::ProcessLoopback {
                executable,
                include_tree,
            } => {
                assert_eq!(executable, "Discord.exe");
                assert!(include_tree);
            }
            other => panic!("听人说话该抓环回，结果是 {other:?}"),
        }
        assert!(!plan.denoise, "数字源本来就干净，不降噪");
        assert!(!plan.hot_update, "听的方向永远译成中文，不许热改");
    }

    #[test]
    fn listen_without_a_target_is_a_clear_error() {
        let mut config = listen_config();
        config.loopback_target = None;
        let err = match plan(&config) {
            Err(err) => err,
            Ok(_) => panic!("没选程序就该报错"),
        };
        assert!(
            err.message.contains("程序"),
            "报错要说人话：{}",
            err.message
        );
    }

    #[test]
    fn listen_can_run_text_only() {
        let mut config = listen_config();
        // 关掉"念出译文"就只剩字幕，省一半 token。
        config.voice = None;
        let plan = plan(&config).expect("纯文字也该能跑");
        assert!(plan.playback_device.is_none());
        assert!(plan.params.voice.is_none());
    }
}
