//! 整机环回：抓某个输出设备上的全部声音。
//!
//! 这是进程环回的退路：系统内部版本 < 20348 的机器上没有按进程抓的接口，
//! 只能整机抓。代价是别的软件出声也会被抓进来（QQ 消息提示音会被当成人在说话），
//! 所以只在没得选的时候用。

use vox_core::ports::PortResult;
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK,
};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};

use crate::com::WinContext;
use crate::devices;
use crate::wave::parse_format;

use super::mic::OpenCapture;
use super::shared::create_stream_event;

/// 打开输出设备的环回采集。`None` 用系统默认输出设备。
pub(crate) fn open_endpoint_loopback(device_name: Option<&str>) -> PortResult<OpenCapture> {
    // 注意方向是 RENDER：环回抓的是输出设备，不是输入设备。
    let device = devices::find_device(devices::RENDER, device_name)?;
    // SAFETY: device 有效；Activate 只创建 IAudioClient。
    let candidate: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.ctx("打开输出设备失败")?;

    // SAFETY: GetMixFormat 返回 COM 分配的格式块；Initialize 之后才释放。
    let (client, info) = unsafe {
        let mix = candidate.GetMixFormat().ctx("读输出设备混音格式失败")?;
        let initialized = (|| -> PortResult<(IAudioClient, crate::wave::WaveInfo)> {
            let parsed = parse_format(mix)?;
            let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
            let client = match crate::client::initialize_min_period(&candidate, flags, mix) {
                Ok(period) => {
                    tracing::debug!(period_frames = period, "整机环回使用 WASAPI 最短共享周期");
                    candidate
                }
                Err(error) => {
                    tracing::debug!(error = %error, "环回最短共享周期不可用，退回系统默认周期");
                    let fallback: IAudioClient = device
                        .Activate(CLSCTX_ALL, None)
                        .ctx("重新打开输出设备失败")?;
                    crate::client::initialize_default_period(&fallback, flags, mix)
                        .ctx("初始化整机环回流失败")?;
                    fallback
                }
            };
            Ok((client, parsed))
        })();
        CoTaskMemFree(Some(mix as *const _));
        initialized?
    };

    let event = create_stream_event()?;
    // SAFETY: client 已初始化；事件在 OpenCapture 存活期间有效。
    unsafe { client.SetEventHandle(event.raw()) }.ctx("绑定环回事件失败")?;
    // SAFETY: client 已初始化，取采集服务接口。
    let capture: IAudioCaptureClient =
        unsafe { client.GetService() }.ctx("获取环回采集接口失败")?;
    // SAFETY: 一切就绪。
    unsafe { client.Start() }.ctx("启动整机环回流失败")?;

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
    #[ignore = "需要真声卡，手动跑：cargo test -p vox-audio-win -- --ignored"]
    fn default_endpoint_loopback_opens() {
        let _com = ComGuard::mta().unwrap();
        let open = open_endpoint_loopback(None).unwrap();
        let f = open.format();
        assert!(f.sample_rate >= 8_000);
        assert!(f.channels >= 1);
    }

    #[test]
    fn unknown_endpoint_name_fails_with_chinese_message() {
        let _com = ComGuard::mta().unwrap();
        let Err(err) = open_endpoint_loopback(Some("不存在的输出 zzz")) else {
            panic!("不存在的设备名不该打开成功");
        };
        assert!(err.message.contains("找不到"), "{}", err.message);
    }
}
