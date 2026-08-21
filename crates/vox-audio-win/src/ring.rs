//! 播放用的无锁环形缓冲：满了丢最旧的，写入永不阻塞。
//!
//! 为什么必须无锁：渲染线程是 WASAPI 的回调节奏（几毫秒一次），一旦它在锁上
//! 等生产者，声音立刻爆音。ARCHITECTURE §6 的规矩是音频回调线程“只准搬数据”，
//! 所以这里的读端只做一次定长复制，没有分配、没有锁、没有日志。
//!
//! 为什么丢最旧的：云端 TTS 是突发到达的，一句话可能几秒钟的音频几百毫秒就到齐。
//! 缓冲满的时候留新丢旧，听感上是“跳一下继续说”；反过来丢新的话，后面的话永远追不上。
//! 这一条是从旧版 playback.py 搬过来的行为。
//!
//! 每个槽位用 `AtomicU32` 存 f32 的位模式。丢最旧的时候写端会推进读指针，
//! 理论上可能和读端撞在同一个槽上；用原子访问就不会有未定义行为，
//! 最坏结果只是溢出瞬间有几个样本是新旧混合的——这种时候本来就在丢数据了。

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// 单生产者单消费者环形缓冲。
pub(crate) struct DropRing {
    slots: Box<[AtomicU32]>,
    /// 累计写入位置，单调递增（64 位在 48 kHz 立体声下也要跑几百万年才会绕回）。
    write: AtomicUsize,
    /// 累计读出位置，单调递增。溢出时写端也会推它。
    read: AtomicUsize,
    /// 累计丢掉的样本数，只用来打日志。
    dropped: AtomicU64,
    /// 丢弃事件次数（一次 `write` 调用算一次），用来决定什么时候告警。
    drop_events: AtomicU64,
}

impl DropRing {
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2);
        let mut slots = Vec::with_capacity(capacity);
        slots.resize_with(capacity, || AtomicU32::new(0));
        Self {
            slots: slots.into_boxed_slice(),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            drop_events: AtomicU64::new(0),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// 当前可读样本数。播放统计也用它换算实时排队时长。
    pub(crate) fn len(&self) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire);
        w.saturating_sub(r)
    }

    /// 写入。返回这次丢掉的样本数（0 表示没溢出）。绝不阻塞。
    pub(crate) fn write(&self, data: &[f32]) -> usize {
        if data.is_empty() {
            return 0;
        }
        let cap = self.capacity();
        // 一次塞进来的比整个缓冲还多，那就只留最后 cap 个——前面的必然要被覆盖。
        let (data, pre_dropped) = if data.len() > cap {
            (&data[data.len() - cap..], data.len() - cap)
        } else {
            (data, 0)
        };

        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        let free = cap - (w - r);
        let mut dropped = pre_dropped;
        if data.len() > free {
            let need = data.len() - free;
            // 推进读指针 = 丢最旧的。用 fetch_max 是因为读端可能同时也在推，
            // 谁靠前听谁的，绝不把读指针往回拉。
            self.read.fetch_max(r + need, Ordering::AcqRel);
            dropped += need;
        }

        for (i, s) in data.iter().enumerate() {
            self.slots[(w + i) % cap].store(s.to_bits(), Ordering::Relaxed);
        }
        // Release 保证读端看到新的 write 时，上面那些槽位的写入也一定可见。
        self.write.store(w + data.len(), Ordering::Release);

        if dropped > 0 {
            self.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
            self.drop_events.fetch_add(1, Ordering::Relaxed);
        }
        dropped
    }

    /// 读出最多 `out.len()` 个样本，剩下的位置补静音。返回真正读到的个数。
    ///
    /// 这是渲染线程唯一做的事：一次定长复制，然后走。
    pub(crate) fn read_into(&self, out: &mut [f32]) -> usize {
        let cap = self.capacity();
        let r = self.read.load(Ordering::Relaxed);
        let w = self.write.load(Ordering::Acquire);
        let avail = w.saturating_sub(r);
        let n = avail.min(out.len());
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = f32::from_bits(self.slots[(r + i) % cap].load(Ordering::Relaxed));
        }
        for slot in out[n..].iter_mut() {
            *slot = 0.0;
        }
        if n > 0 {
            self.read.fetch_max(r + n, Ordering::AcqRel);
        }
        n
    }

    /// 丢掉所有待播内容。打断说话时用。
    pub(crate) fn clear(&self) {
        let w = self.write.load(Ordering::Acquire);
        self.read.fetch_max(w, Ordering::AcqRel);
    }

    /// 累计丢样本数。
    pub(crate) fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 累计丢弃事件次数。
    pub(crate) fn drop_events(&self) -> u64 {
        self.drop_events.load(Ordering::Relaxed)
    }
}

/// 告警节流：第一次丢就说一声，之后每 25 次说一次。
///
/// 跟旧版 playback.py / session.py 一个节奏。既能第一时间发现问题，
/// 又不会在持续溢出时把日志刷爆。
pub(crate) fn should_warn(drop_events: u64) -> bool {
    drop_events == 1 || (drop_events > 0 && drop_events.is_multiple_of(25))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrip() {
        let ring = DropRing::new(8);
        assert_eq!(ring.write(&[1.0, 2.0, 3.0]), 0);
        assert_eq!(ring.len(), 3);
        let mut out = [0.0f32; 4];
        assert_eq!(ring.read_into(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0, 0.0]);
        assert_eq!(ring.len(), 0);
    }

    #[test]
    fn overflow_drops_oldest_and_keeps_newest() {
        let ring = DropRing::new(4);
        assert_eq!(ring.write(&[1.0, 2.0, 3.0, 4.0]), 0);
        // 再写 2 个 -> 丢掉最旧的 2 个
        assert_eq!(ring.write(&[5.0, 6.0]), 2);
        let mut out = [0.0f32; 4];
        assert_eq!(ring.read_into(&mut out), 4);
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0]);
        assert_eq!(ring.dropped_samples(), 2);
        assert_eq!(ring.drop_events(), 1);
    }

    #[test]
    fn write_larger_than_capacity_keeps_tail() {
        let ring = DropRing::new(3);
        assert_eq!(ring.write(&[1.0, 2.0, 3.0, 4.0, 5.0]), 2);
        let mut out = [0.0f32; 3];
        assert_eq!(ring.read_into(&mut out), 3);
        assert_eq!(out, [3.0, 4.0, 5.0]);
    }

    #[test]
    fn underrun_pads_with_silence() {
        let ring = DropRing::new(8);
        ring.write(&[0.5]);
        let mut out = [9.0f32; 3];
        assert_eq!(ring.read_into(&mut out), 1);
        assert_eq!(out, [0.5, 0.0, 0.0]);
    }

    #[test]
    fn clear_drops_everything_pending() {
        let ring = DropRing::new(8);
        ring.write(&[1.0, 2.0, 3.0]);
        ring.clear();
        assert_eq!(ring.len(), 0);
        let mut out = [7.0f32; 2];
        assert_eq!(ring.read_into(&mut out), 0);
        assert_eq!(out, [0.0, 0.0]);
    }

    #[test]
    fn warn_on_first_then_every_25th() {
        assert!(should_warn(1));
        assert!(!should_warn(2));
        assert!(!should_warn(24));
        assert!(should_warn(25));
        assert!(should_warn(50));
        assert!(!should_warn(51));
        assert!(!should_warn(0));
    }

    #[test]
    fn wraps_around_many_times() {
        let ring = DropRing::new(5);
        let mut out = [0.0f32; 3];
        for i in 0..1000u32 {
            ring.write(&[i as f32, i as f32 + 0.5, i as f32 + 0.25]);
            assert_eq!(ring.read_into(&mut out), 3);
            assert_eq!(out, [i as f32, i as f32 + 0.5, i as f32 + 0.25]);
        }
        assert_eq!(ring.dropped_samples(), 0);
    }
}
