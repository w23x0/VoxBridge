//! 麦克风采集：WASAPI 共享模式，事件驱动。
//!
//! 共享模式而不是独占：独占能少几毫秒延迟，但会把设备占死，别的软件（包括
//! VRChat 自己）就用不了麦了。语音翻译这点延迟无所谓，能共存更重要。
//!
//! 格式直接用 `GetMixFormat` 的结果，不去跟设备较劲。共享模式下混音格式是
//! 唯一保证能 `Initialize` 成功的格式，协商到什么就把什么报给上层，
//! 由流水线那边负责转到 16 kHz。

use vox_core::ports::{CaptureFormat, PortResult};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};

use crate::com::{OwnedHandle, WinContext};
use crate::devices;
use crate::wave::{parse_format, WaveInfo};

use super::shared::create_stream_event;

/// 打开的采集流。全部接口都属于调用它的那个线程。
pub(crate) struct OpenCapture {
    pub(crate) client: IAudioClient,
    pub(crate) capture: IAudioCaptureClient,
    pub(crate) event: OwnedHandle,
    pub(crate) info: WaveInfo,
}

impl OpenCapture {
    pub(crate) fn format(&self) -> CaptureFormat {
        CaptureFormat {
            sample_rate: self.info.sample_rate,
            channels: self.info.channels,
        }
    }
}

/// 按设备名打开麦克风。`None` 用系统默认输入设备。
pub(crate) fn open_microphone(device_name: Option<&str>) -> PortResult<OpenCapture> {
    let device = devices::find_device(devices::CAPTURE, device_name)?;
    // SAFETY: device 有效；Activate 只创建 IAudioClient，不带激活参数。
    let candidate: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.ctx("打开麦克风失败")?;

    // SAFETY: GetMixFormat 返回 COM 分配的格式块，解析完立刻释放；
    // Initialize 期间指针必须有效，所以释放放在 Initialize 之后。
    let (client, info) = unsafe {
        let mix = candidate.GetMixFormat().ctx("读麦克风混音格式失败")?;
        let initialized = (|| -> PortResult<(IAudioClient, WaveInfo)> {
            let parsed = parse_format(mix)?;
            let client = match crate::client::initialize_min_period(
                &candidate,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                mix,
            ) {
                Ok(period) => {
                    tracing::debug!(period_frames = period, "麦克风使用 WASAPI 最短共享周期");
                    candidate
                }
                Err(error) => {
                    tracing::debug!(error = %error, "最短共享周期不可用，退回系统默认周期");
                    let fallback: IAudioClient = device
                        .Activate(CLSCTX_ALL, None)
                        .ctx("重新打开麦克风失败")?;
                    crate::client::initialize_default_period(
                        &fallback,
                        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                        mix,
                    )
                    .ctx("初始化麦克风流失败")?;
                    fallback
                }
            };
            Ok((client, parsed))
        })();
        CoTaskMemFree(Some(mix as *const _));
        initialized?
    };

    let event = create_stream_event()?;
    // SAFETY: client 已初始化；事件句柄在 OpenCapture 存活期间一直有效，
    // 而 client 会先于它被丢弃（结构体字段声明顺序：client 在 event 之前）。
    unsafe { client.SetEventHandle(event.raw()) }.ctx("绑定麦克风事件失败")?;
    // SAFETY: client 已初始化，取采集服务接口。
    let capture: IAudioCaptureClient =
        unsafe { client.GetService() }.ctx("获取麦克风采集接口失败")?;
    // SAFETY: 一切就绪，开始走流。
    unsafe { client.Start() }.ctx("启动麦克风流失败")?;

    Ok(OpenCapture {
        client,
        capture,
        event,
        info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::com::ComGuard;

    #[test]
    #[ignore = "需要真麦克风，手动跑：cargo test -p vox-audio-win -- --ignored"]
    fn default_microphone_opens_and_reports_format() {
        let _com = ComGuard::mta().unwrap();
        let open = open_microphone(None).unwrap();
        let f = open.format();
        assert!(f.sample_rate >= 8_000 && f.sample_rate <= 192_000, "{f:?}");
        assert!(f.channels >= 1);
    }

    #[test]
    fn unknown_device_name_fails_with_chinese_message() {
        let _com = ComGuard::mta().unwrap();
        // OpenCapture 里全是 COM 接口，不方便 derive Debug，所以手动解构而不是 unwrap_err。
        let Err(err) = open_microphone(Some("不存在的麦 zzz")) else {
            panic!("不存在的设备名不该打开成功");
        };
        assert!(err.message.contains("找不到"), "{}", err.message);
    }
}
