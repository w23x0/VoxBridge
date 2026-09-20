//! 声道铺开：内核只给单声道，设备大多要立体声甚至 7.1。
//!
//! 复制到每个声道而不是"左声道有、右声道补零"：语音不需要声场，补零会让某些设备
//! 只有一边出声。这条行为是从 Windows 侧的 `wave.rs` 原样搬过来的，两个平台共用。

/// 单声道铺到多声道（交错）。
pub fn duplicate_mono(mono: &[f32], channels: u16, out: &mut Vec<f32>) {
    let channels = channels.max(1) as usize;
    out.reserve(mono.len() * channels);
    if channels == 1 {
        out.extend_from_slice(mono);
        return;
    }
    for &sample in mono {
        for _ in 0..channels {
            out.push(sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_to_every_channel() {
        let mut out = Vec::new();
        duplicate_mono(&[1.0, -1.0], 2, &mut out);
        assert_eq!(out, vec![1.0, 1.0, -1.0, -1.0]);
    }

    #[test]
    fn mono_is_a_passthrough() {
        let mut out = Vec::new();
        duplicate_mono(&[0.25], 1, &mut out);
        assert_eq!(out, vec![0.25]);
    }

    #[test]
    fn zero_channels_is_treated_as_mono() {
        let mut out = Vec::new();
        duplicate_mono(&[0.5], 0, &mut out);
        assert_eq!(out, vec![0.5]);
    }
}
