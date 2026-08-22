//! 输出采样率的探测顺序。
//!
//! 顺序是 设备默认 → 24000 → 48000 → 44100，从旧版 playback.py 原样搬过来。
//! 为什么设备默认排第一：共享模式下设备默认率是唯一保证不经过系统重采样的选择，
//! 走它音质最稳；我们自己把 24 kHz 抬上去，比让驱动去凑一个它不喜欢的率更可控。

/// 内核给播放端的固定采样率。
pub(crate) const KERNEL_OUTPUT_RATE: u32 = 24_000;

/// 探测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RateChoice {
    /// 最终用来打开设备的采样率。
    pub(crate) rate: u32,
}

/// 按固定顺序探测，返回第一个设备接受的率。
///
/// `supported` 是探测回调（实现里是 `IsFormatSupported`）。一个都不接受时
/// 退回设备默认率——那种情况下 `Initialize` 大概率也会失败，让调用方去报错，
/// 这里不擅自换策略。
pub(crate) fn choose_output_rate(
    device_default: u32,
    mut supported: impl FnMut(u32) -> bool,
) -> RateChoice {
    let mut candidates = [device_default, KERNEL_OUTPUT_RATE, 48_000, 44_100];
    let mut chosen = None;
    for i in 0..candidates.len() {
        let rate = candidates[i];
        if rate == 0 || candidates[..i].contains(&rate) {
            continue;
        }
        if supported(rate) {
            chosen = Some(rate);
            break;
        }
    }
    // 保证返回值有意义：探测全灭时用设备默认率，再兜底 48 kHz。
    candidates[0] = if device_default == 0 {
        48_000
    } else {
        device_default
    };
    let rate = chosen.unwrap_or(candidates[0]);
    RateChoice { rate }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_default_wins_when_supported() {
        let c = choose_output_rate(48_000, |r| r == 48_000 || r == 24_000);
        assert_eq!(c, RateChoice { rate: 48_000 });
    }

    #[test]
    fn falls_to_kernel_output_rate_when_device_default_refused() {
        let c = choose_output_rate(44_100, |r| r == 24_000);
        assert_eq!(c, RateChoice { rate: 24_000 });
    }

    #[test]
    fn probe_order_is_default_then_24k_then_48k_then_441k() {
        let mut seen = Vec::new();
        let _ = choose_output_rate(96_000, |r| {
            seen.push(r);
            false
        });
        assert_eq!(seen, vec![96_000, 24_000, 48_000, 44_100]);
    }

    #[test]
    fn duplicate_default_is_not_probed_twice() {
        let mut seen = Vec::new();
        let _ = choose_output_rate(24_000, |r| {
            seen.push(r);
            false
        });
        assert_eq!(seen, vec![24_000, 48_000, 44_100]);
    }

    #[test]
    fn nothing_supported_falls_back_to_device_default() {
        let c = choose_output_rate(44_100, |_| false);
        assert_eq!(c, RateChoice { rate: 44_100 });
    }

    #[test]
    fn zero_device_default_is_skipped_and_backfilled() {
        let mut seen = Vec::new();
        let c = choose_output_rate(0, |r| {
            seen.push(r);
            false
        });
        assert_eq!(seen, vec![24_000, 48_000, 44_100]);
        assert_eq!(c.rate, 48_000);
    }

    #[test]
    fn last_resort_441k_is_reachable() {
        let c = choose_output_rate(192_000, |r| r == 44_100);
        assert_eq!(c.rate, 44_100);
    }
}
