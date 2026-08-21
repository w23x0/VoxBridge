//! `Clock` 的 Windows 实现。
//!
//! 内核不读系统时钟，时间全从这儿来。两件事分开取：
//! - `now_ms` 用单调时钟（`Instant`），改系统时间不会让字幕的 TTL 算乱；
//! - `stamp` 要的是**本地日期**（用量按天/按月分桶），必须走挂钟时间。

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use vox_core::ports::Clock;
use vox_core::usage::Stamp;
use windows::Win32::System::SystemInformation::GetLocalTime;

pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        // 单调：`Instant` 之差永远不为负，转 u64 安全。
        self.origin.elapsed().as_millis() as u64
    }

    fn stamp(&self) -> Stamp {
        // GetLocalTime 已经按用户的时区和夏令时算好了，比自己拿 UTC 再套偏移可靠。
        let local = unsafe { GetLocalTime() };
        Stamp {
            unix_secs: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            year: local.wYear as i32,
            month: local.wMonth as u32,
            day: local.wDay as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_ms_is_monotonic() {
        let clock = SystemClock::new();
        let a = clock.now_ms();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = clock.now_ms();
        assert!(b >= a, "单调时钟不该往回走：{a} -> {b}");
    }

    #[test]
    fn stamp_has_a_plausible_local_date() {
        let s = SystemClock::new().stamp();
        assert!(s.year >= 2024, "年份不对：{}", s.year);
        assert!((1..=12).contains(&s.month), "月份不对：{}", s.month);
        assert!((1..=31).contains(&s.day), "日不对：{}", s.day);
        // 2024-01-01 的 unix 秒。时钟没坏的话一定在这之后。
        assert!(s.unix_secs > 1_704_067_200, "unix 秒不对：{}", s.unix_secs);
    }
}
