//! 实时链路的延迟统计。
//!
//! 这里只保存时间和计数，不保存音频或字幕内容。每个指标保留最近 64 个样本，
//! 对外暴露 last / p50 / p95；冷启动连接时间单独记录，不混进逐轮延迟。

use std::collections::VecDeque;

use serde::Serialize;

const WINDOW: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MetricSummary {
    pub last_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    pub samples: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LatencySnapshot {
    /// TCP/TLS/WebSocket + session.update 发出的冷启动时间。
    pub connect_ms: Option<u64>,
    /// 从开始连接到收到 session.updated 的时间。
    pub session_ready_ms: Option<u64>,
    pub input_queue: MetricSummary,
    pub upload_send: MetricSummary,
    /// 原始语音起点到服务端 VAD 上升沿回到本机。
    pub server_vad: MetricSummary,
    /// 原始语音起点到首个可显示译文。
    pub first_text: MetricSummary,
    /// 原始语音起点到首个译音包到达。
    pub first_audio: MetricSummary,
    /// 原始语音起点到首个译音样本被 WASAPI 渲染线程取走。
    pub first_playback: MetricSummary,
    /// 原始语音起点到 response.done。
    pub turn_complete: MetricSummary,
    pub completed_turns: u64,
    pub input_queue_depth: u32,
    pub input_queue_oldest_ms: u64,
    pub playback_queue_ms: u64,
    pub processed_chunks: u64,
    pub dropped_chunks: u64,
}

#[derive(Debug, Clone, Default)]
struct RollingMetric {
    values: VecDeque<u64>,
}

impl RollingMetric {
    fn push(&mut self, value: u64) {
        if self.values.len() == WINDOW {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    fn summary(&self) -> MetricSummary {
        if self.values.is_empty() {
            return MetricSummary::default();
        }
        let mut sorted: Vec<u64> = self.values.iter().copied().collect();
        sorted.sort_unstable();
        let percentile = |numerator: usize, denominator: usize| {
            let index = ((sorted.len() - 1) * numerator).div_ceil(denominator);
            sorted[index.min(sorted.len() - 1)]
        };
        MetricSummary {
            last_ms: self.values.back().copied(),
            p50_ms: Some(percentile(50, 100)),
            p95_ms: Some(percentile(95, 100)),
            samples: self.values.len() as u32,
        }
    }
}

/// 只由一条流水线工作线程持有，无需加锁。
#[derive(Debug, Default)]
pub(crate) struct LatencyTracker {
    connect_ms: Option<u64>,
    session_ready_ms: Option<u64>,
    input_queue: RollingMetric,
    upload_send: RollingMetric,
    server_vad: RollingMetric,
    first_text: RollingMetric,
    first_audio: RollingMetric,
    first_playback: RollingMetric,
    turn_complete: RollingMetric,
    completed_turns: u64,
}

impl LatencyTracker {
    pub(crate) fn set_connect(&mut self, value: u64) {
        self.connect_ms = Some(value);
        self.session_ready_ms = None;
    }

    pub(crate) fn set_session_ready(&mut self, value: u64) {
        self.session_ready_ms = Some(value);
    }

    pub(crate) fn input_queue(&mut self, value: u64) {
        self.input_queue.push(value);
    }

    pub(crate) fn upload_send(&mut self, value: u64) {
        self.upload_send.push(value);
    }

    pub(crate) fn server_vad(&mut self, value: u64) {
        self.server_vad.push(value);
    }

    pub(crate) fn first_text(&mut self, value: u64) {
        self.first_text.push(value);
    }

    pub(crate) fn first_audio(&mut self, value: u64) {
        self.first_audio.push(value);
    }

    pub(crate) fn first_playback(&mut self, value: u64) {
        self.first_playback.push(value);
    }

    pub(crate) fn turn_complete(&mut self, value: u64) {
        self.turn_complete.push(value);
        self.completed_turns += 1;
    }

    pub(crate) fn snapshot(
        &self,
        input_queue_depth: usize,
        input_queue_oldest_ms: u64,
        playback_queue_ms: u64,
        processed_chunks: u64,
        dropped_chunks: u64,
    ) -> LatencySnapshot {
        LatencySnapshot {
            connect_ms: self.connect_ms,
            session_ready_ms: self.session_ready_ms,
            input_queue: self.input_queue.summary(),
            upload_send: self.upload_send.summary(),
            server_vad: self.server_vad.summary(),
            first_text: self.first_text.summary(),
            first_audio: self.first_audio.summary(),
            first_playback: self.first_playback.summary(),
            turn_complete: self.turn_complete.summary(),
            completed_turns: self.completed_turns,
            input_queue_depth: input_queue_depth.min(u32::MAX as usize) as u32,
            input_queue_oldest_ms,
            playback_queue_ms,
            processed_chunks,
            dropped_chunks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_window_reports_last_median_and_p95() {
        let mut metric = RollingMetric::default();
        for value in 1..=100 {
            metric.push(value);
        }
        let summary = metric.summary();
        assert_eq!(summary.samples, 64);
        assert_eq!(summary.last_ms, Some(100));
        assert_eq!(summary.p50_ms, Some(69));
        assert_eq!(summary.p95_ms, Some(97));
    }

    #[test]
    fn empty_metric_serializes_as_unknown_instead_of_fake_zero() {
        let summary = MetricSummary::default();
        assert_eq!(summary.last_ms, None);
        assert_eq!(summary.samples, 0);
    }
}
