//! 字幕变更检测器：把"逐 token 涌进来的字幕"压成"变了 / 没变"。
//!
//! 两条输入：
//!
//! - **事件口**（[`Transcripts::note`] / [`Transcripts::cleared`]）：芯的事件监听器回调进来，
//!   只记 `confirmed` / `done` 两个**事件里才有**的字段（快照里那两格）；
//! - **账本口**（[`Transcripts::observe`]）：ticker 每次读一遍账本（字幕文本 + 流水线状态），
//!   和上一次发出去的样子比——**只有真的变了才涨 `revision`**，调用方据此决定发不发通知
//!   （设计稿 §2.4.2 的去抖：逐 token 的 delta 不该变成逐 token 的通知）。
//!
//! 设计稿写的是"监听器里只置一个原子脏标志，ticker 检查它再决定读不读账本"。这里改成
//! **每个 tick 都读一次**，理由是字幕的字会**自然过期**（`char_ttl_ms` 到了就透明、快照里的
//! `text` 就短了）——那一类变化没有任何事件，只看脏标志会漏掉它，而 ticker 只在有订阅流时
//! 才跑、一拍一次锁，代价可以忽略。去抖的本体（"只有变了才发"）一字未改。
//!
//! **锁序**（防死锁，唯一条）：绝不可以在持有本结构的锁时调账本/芯。芯的事件回调可能在
//! 持有状态锁时进来（[`Transcripts::note`] 会在那条路径上拿本结构的锁），所以调用方必须
//! **先**把账本读出来，**再**锁本结构（见 `session.rs::poll_resources`）。

use std::collections::BTreeMap;

use vox_core::event::PipelineState;
use vox_core::subtitle::Track;

/// 每条轨道上"最近一条 delta"说了什么（事件才有，账本里读不到）。
#[derive(Default)]
struct TrackState {
    /// 不再会变的那段（`Event::SubtitleDelta.confirmed`），逐条覆盖：没有就是 `None`。
    confirmed: Option<String>,
    /// 最近一条 delta 是不是"这一段说完了"。
    done: bool,
}

/// 一条会话上一次**发出去**的样子。四项任一不同 = 变了。
struct Fingerprint {
    revision: u64,
    text: String,
    state: PipelineState,
    confirmed: Option<String>,
    done: bool,
    updated_at_ms: u64,
}

/// 字幕变更检测器。一个控制面进程一份（`LedgerBackend` 持有，事件监听器与 ticker 共用）。
#[derive(Default)]
pub struct Transcripts {
    tracks: [TrackState; 2],
    /// key = 会话 handle（服务端签发的那个，永不复用）。
    seen: BTreeMap<String, Fingerprint>,
    /// 上一次看见的"活着的资源集合"（顺序也要一样：它决定 `list_revision` 涨不涨）。
    live: Vec<String>,
    list_revision: u64,
}

impl Transcripts {
    /// 事件口：来了一条字幕 delta。`confirmed` / `done` **逐条覆盖**（`None` 就是没有可用前缀）。
    ///
    /// 可能在芯的状态锁里被调用：这里只碰自己的数据，不回调任何东西。
    pub fn note(&mut self, track: Track, confirmed: Option<&str>, done: bool) {
        let slot = &mut self.tracks[slot(track)];
        slot.confirmed = confirmed.map(str::to_string);
        slot.done = done;
    }

    /// 事件口：这一条轨道的字幕被清空了。
    pub fn cleared(&mut self, track: Track) {
        self.tracks[slot(track)] = TrackState::default();
    }

    /// 账本口：一条会话此刻的样子。`true` = 和上一次不一样（`revision` 已经涨了）。
    ///
    /// 第一次看见一个 handle 只是**登记**（`revision` 从 0 起，不算"变了"）——刚订阅上来的
    /// 客户端不需要为"它本来就有字"收一条通知，它自己会 `resources/read`。
    pub fn observe(
        &mut self,
        handle: &str,
        track: Track,
        text: &str,
        state: PipelineState,
        now_ms: u64,
    ) -> bool {
        let slot = &self.tracks[slot(track)];
        let confirmed = slot.confirmed.clone();
        let done = slot.done;

        match self.seen.get_mut(handle) {
            None => {
                self.seen.insert(
                    handle.to_string(),
                    Fingerprint {
                        revision: 0,
                        text: text.to_string(),
                        state,
                        confirmed,
                        done,
                        updated_at_ms: now_ms,
                    },
                );
                false
            }
            Some(seen) => {
                if seen.text == text
                    && seen.state == state
                    && seen.confirmed == confirmed
                    && seen.done == done
                {
                    return false;
                }
                seen.text.clear();
                seen.text.push_str(text);
                seen.state = state;
                seen.confirmed = confirmed;
                seen.done = done;
                seen.revision += 1;
                seen.updated_at_ms = now_ms;
                true
            }
        }
    }

    /// 账本口：现在活着的 handle 有哪些。集合变了就涨 `list_revision`（会话开/关 = 资源增删），
    /// 顺手把已经关掉的 handle 的指纹扔掉（handle 永不复用）。
    pub fn live(&mut self, handles: &[&str]) {
        if self.live.len() == handles.len()
            && self.live.iter().zip(handles).all(|(seen, now)| seen == now)
        {
            return;
        }
        self.live = handles.iter().map(|handle| (*handle).to_string()).collect();
        self.list_revision += 1;
        self.seen
            .retain(|handle, _| handles.contains(&handle.as_str()));
    }

    /// 这条会话现在的 revision（不认识的 handle → 0）。
    pub fn revision(&self, handle: &str) -> u64 {
        self.seen.get(handle).map_or(0, |seen| seen.revision)
    }

    /// 活着的资源集合涨到第几版了（订阅流拿它当"列表变没变"的水位）。
    pub fn list_revision(&self) -> u64 {
        self.list_revision
    }

    /// 这条会话上一次观察到的 `confirmed`（没有可用前缀就是 `None`）。
    ///
    /// 只有 [`Transcripts::observe`] 登记过的 handle 才有；调用方（`resources/read`、ticker）
    /// 都是**先 observe 再读**，所以活着的会话一定读得到。
    pub fn confirmed(&self, handle: &str) -> Option<&str> {
        self.seen
            .get(handle)
            .and_then(|seen| seen.confirmed.as_deref())
    }

    /// 这条会话上一次观察到的 `last_delta_done`。
    pub fn last_delta_done(&self, handle: &str) -> bool {
        self.seen.get(handle).is_some_and(|seen| seen.done)
    }

    /// 上一次观察到变化的时间（账本的单调毫秒时钟）。
    pub fn updated_at_ms(&self, handle: &str) -> u64 {
        self.seen.get(handle).map_or(0, |seen| seen.updated_at_ms)
    }
}

/// `Track` → 数组下标。两条轨道各一格，不写 `BTreeMap`（编译期就知道只有两个）。
fn slot(track: Track) -> usize {
    match track {
        Track::Speak => 0,
        Track::Listen => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts() -> (Track, Track) {
        (Track::Speak, Track::Listen)
    }

    #[test]
    fn only_a_real_change_bumps_the_revision() {
        let (speak, _) = texts();
        let mut transcripts = Transcripts::default();

        // 第一次看见 = 登记，不算变化。
        assert!(!transcripts.observe("s_a", speak, "你好", PipelineState::Ready, 10));
        assert_eq!(transcripts.revision("s_a"), 0);
        assert_eq!(transcripts.updated_at_ms("s_a"), 10);

        // 一模一样 → 不涨。
        assert!(!transcripts.observe("s_a", speak, "你好", PipelineState::Ready, 20));
        assert_eq!(transcripts.revision("s_a"), 0);
        assert_eq!(transcripts.updated_at_ms("s_a"), 10);

        // 文本变了 → 涨。
        assert!(transcripts.observe("s_a", speak, "你好，很高兴", PipelineState::Ready, 30));
        assert_eq!(transcripts.revision("s_a"), 1);
        assert_eq!(transcripts.updated_at_ms("s_a"), 30);

        // 状态变了（说话中）→ 也涨（设计稿：text/confirmed/state 任一变了就发）。
        assert!(transcripts.observe("s_a", speak, "你好，很高兴", PipelineState::Active, 40));
        assert_eq!(transcripts.revision("s_a"), 2);

        // confirmed 变了 → 也涨。
        transcripts.note(speak, Some("你好"), false);
        assert!(transcripts.observe("s_a", speak, "你好，很高兴", PipelineState::Active, 50));
        assert_eq!(transcripts.revision("s_a"), 3);
        assert_eq!(transcripts.confirmed("s_a"), Some("你好"));
        assert!(!transcripts.last_delta_done("s_a"));
    }

    #[test]
    fn a_new_handle_starts_from_zero_and_a_closed_one_is_forgotten() {
        let (speak, listen) = texts();
        let mut transcripts = Transcripts::default();

        assert!(!transcripts.observe("s_a", speak, "甲", PipelineState::Ready, 1));
        transcripts.observe("s_b", listen, "乙", PipelineState::Ready, 1);
        assert!(transcripts.observe("s_a", speak, "甲乙", PipelineState::Ready, 2));
        assert_eq!(transcripts.revision("s_a"), 1);

        transcripts.live(&["s_a", "s_b"]);
        assert_eq!(transcripts.list_revision(), 1);
        transcripts.live(&["s_a", "s_b"]);
        assert_eq!(transcripts.list_revision(), 1, "集合没变就不涨");

        // 关掉 s_b：集合变了，且它的指纹被扔掉（handle 永不复用，留着只是垃圾）。
        transcripts.live(&["s_a"]);
        assert_eq!(transcripts.list_revision(), 2);
        assert_eq!(transcripts.revision("s_b"), 0);

        // 两个 handle 互不干扰。
        assert_eq!(transcripts.revision("s_a"), 1);
        assert!(transcripts.observe("s_a", speak, "丙", PipelineState::Ready, 3));
        assert_eq!(transcripts.revision("s_a"), 2);
    }

    #[test]
    fn the_two_tracks_do_not_bleed_into_each_other() {
        let (speak, listen) = texts();
        let mut transcripts = Transcripts::default();

        transcripts.note(speak, Some("对外"), false);
        transcripts.note(listen, Some("听人"), true);
        transcripts.observe("s_a", speak, "", PipelineState::Ready, 1);
        transcripts.observe("s_b", listen, "", PipelineState::Ready, 1);

        assert_eq!(transcripts.confirmed("s_a"), Some("对外"));
        assert_eq!(transcripts.confirmed("s_b"), Some("听人"));
        assert!(!transcripts.last_delta_done("s_a"));
        assert!(transcripts.last_delta_done("s_b"));

        // 清空一条轨道：**已经发出去的那份不变**（清空要等下一次 observe 才发现，
        // 那时 text 会变空 → 涨 revision → 客户端才收得到通知）。
        transcripts.cleared(speak);
        assert_eq!(transcripts.confirmed("s_a"), Some("对外"));
        assert_eq!(transcripts.confirmed("s_b"), Some("听人"));
        assert!(transcripts.observe("s_a", speak, "", PipelineState::Ready, 2));
        assert_eq!(transcripts.confirmed("s_a"), None);
        assert_eq!(transcripts.confirmed("s_b"), Some("听人"));
    }
}
