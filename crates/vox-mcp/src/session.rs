//! 会话生命周期与两段式确认：handle 注册表、compose token，以及 [`ControlBackend`] 的默认实现。
//!
//! 协议**没有 session**（2026-07-28 删了）：handle 由服务端签发、当普通参数显式传。所以这里有
//! 两张内存表——控制面开出来的会话 handle，与 dry-run 签出去的 compose token——两张都随进程消失，
//! 都不落盘（它们不是用户数据，是工具会话态）。
//!
//! 两个动作的失败语义（§2.1.3-④⑤）：
//!
//! - `session_open`：超时/失败**不留孤儿**（返回前一定 `stop`，走握手）。
//! - `session_close`：**不需要任何授权位**——关麦永远不该被授权位挡住，否则用户一关授权位就
//!   再也关不掉麦克风。已关过的 handle → 成功 + `stopped: false`（幂等）。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use serde_json::{json, Value};
use vox_core::event::{Event, PipelineState};
use vox_core::runtime::Listener;

use crate::actions::{
    action_by_id, ActionId, DomainError, DomainErrorCode, EndpointId, Permission,
    COMPOSE_TOKEN_TTL_MS,
};
use crate::endpoints;
use crate::handlers::{CallFailure, ControlBackend};
use crate::ledger::{Grants, Ledger};
use crate::resources::{self, ResourceState, ResourceTick};
use crate::transcript::Transcripts;

/// `session_open` 等就绪时的轮询节拍。20 ms = 芯的一个音频块（`INPUT_BLOCK_MS`），比它更密
/// 只是空转。
const READY_POLL: StdDuration = StdDuration::from_millis(20);

/// 已经关过的 handle 留多少个（只为"幂等回 `stopped:false`"这一件事，多了没意义）。
const CLOSED_HANDLES: usize = 64;

/// compose 的一次性 token（§2.1.3-③）：`c_` + 16 字节 `getrandom` 的 hex，绑定
/// **(端点, 清单文本)**，TTL 120 s，**用过即废**。
///
/// 绑定用的是清单的**线上文本**（逐字节比），不是它的哈希：本 crate 不许引新依赖，而精确比较
/// 比哈希更强——哈希只会多一层碰撞面。每个端点最多留一个待用 token（新的 dry-run 覆盖旧的），
/// 所以这张表最多两格。
#[derive(Default)]
pub struct Tokens {
    pending: BTreeMap<EndpointId, Pending>,
}

struct Pending {
    token: String,
    manifest: String,
    expires_at_ms: u64,
}

impl Tokens {
    /// 签一个 token。返回 `(token, expires_in_ms)`。
    pub fn sign(&mut self, endpoint: EndpointId, manifest: String, now_ms: u64) -> (String, u64) {
        let token = mint("c_", 16);
        self.pending.insert(
            endpoint,
            Pending {
                token: token.clone(),
                manifest,
                expires_at_ms: now_ms + COMPOSE_TOKEN_TTL_MS,
            },
        );
        (token, COMPOSE_TOKEN_TTL_MS)
    }

    /// 核销一个 token：匹配 + 未过期 + 未用过。
    ///
    /// **失败也把待用的那个作废**（fail-closed）：一次用错的 `apply` 之后，客户端必须重新
    /// dry-run 拿新 token，而不是拿旧 token 反复试。失败返回"还剩多久"（0 = 已经彻底没用了）。
    pub fn redeem(
        &mut self,
        endpoint: EndpointId,
        token: Option<&str>,
        manifest: &str,
        now_ms: u64,
    ) -> Result<(), u64> {
        let Some(pending) = self.pending.remove(&endpoint) else {
            return Err(0);
        };
        let expires_in_ms = pending.expires_at_ms.saturating_sub(now_ms);
        let matched = token == Some(pending.token.as_str())
            && pending.manifest == manifest
            && now_ms < pending.expires_at_ms;
        if matched {
            Ok(())
        } else {
            Err(expires_in_ms)
        }
    }
}

/// 控制面会话的 handle 注册表。两个方向都要：`open` 查"这条腿现在有没有 handle"（幂等），
/// `close` 查"这个 handle 认不认识"与"是不是已经关过了"（幂等 + `stopped:false`）。
#[derive(Default)]
pub struct Sessions {
    open: BTreeMap<EndpointId, String>,
    closed: BTreeMap<String, EndpointId>,
    closed_order: VecDeque<String>,
}

impl Sessions {
    /// 这条腿的 handle：控制面已经签过就复用，否则现签一个。
    ///
    /// 界面自己开起来的会话也走这里：账本只回答"这条腿在不在跑"，handle 是控制面的记账。
    pub fn handle_for(&mut self, endpoint: EndpointId) -> String {
        if let Some(handle) = self.open.get(&endpoint) {
            return handle.clone();
        }
        let handle = mint("s_", 8);
        self.open.insert(endpoint, handle.clone());
        handle
    }

    /// 认领一个要关的 handle：从在册表里摘掉并记进"关过的"。不认识的返回 `None`。
    fn forget(&mut self, session: &str) -> Option<EndpointId> {
        let endpoint = self
            .open
            .iter()
            .find(|(_, handle)| handle.as_str() == session)
            .map(|(endpoint, _)| *endpoint)?;
        self.open.remove(&endpoint);
        self.closed.insert(session.to_string(), endpoint);
        self.closed_order.push_back(session.to_string());
        while self.closed_order.len() > CLOSED_HANDLES {
            if let Some(oldest) = self.closed_order.pop_front() {
                self.closed.remove(&oldest);
            }
        }
        Some(endpoint)
    }

    /// 这个 handle 之前关过吗（幂等回 `stopped:false` 用）。
    fn was_closed(&self, session: &str) -> Option<EndpointId> {
        self.closed.get(session).copied()
    }

    /// 现在**活着**的会话，顺序 = 端点顺序（`BTreeMap` 的键序，稳定）。资源面的 `resources/list`
    /// 就是它：一个活着的会话 = 一条字幕资源。
    pub fn live(&self) -> impl Iterator<Item = (EndpointId, &str)> {
        self.open
            .iter()
            .map(|(endpoint, handle)| (*endpoint, handle.as_str()))
    }

    /// 这个 handle 现在属于哪条腿。不认识的（从没签发过 / 已经关了）→ `None`。
    pub fn endpoint_of(&self, session: &str) -> Option<EndpointId> {
        self.open
            .iter()
            .find(|(_, handle)| handle.as_str() == session)
            .map(|(endpoint, _)| *endpoint)
    }
}

/// `session_open`：按当前清单把端点真的跑起来（已在跑则幂等返回现有 handle）。
pub fn open(
    ledger: &dyn Ledger,
    grants: &dyn Grants,
    sessions: &mut Sessions,
    endpoint: EndpointId,
    wait_ready_ms: u32,
) -> Result<Value, CallFailure> {
    // 闸门①：用户授权位。按端点取（`speak` 要麦克风、`listen` 抓程序音），不写成并集。
    for permission in action_by_id(ActionId::SessionOpen)
        .permissions
        .for_endpoint(Some(endpoint))
    {
        if !grants.user_granted(*permission) {
            return Err(DomainError::with_detail(
                DomainErrorCode::PermissionDenied,
                "用户没有允许控制面用这个设备",
                json!({
                    "permission": permission.as_str(),
                    "gate": "vox_user",
                    "hint": hint_of(*permission),
                }),
            )
            .into());
        }
    }

    // 闸门②：本机做不做得到（清单过不过两道闸）。
    let manifest = endpoints::manifest(ledger, endpoint)?;
    let caps = ledger.capabilities();
    let mut errors = manifest.validate().err().unwrap_or_default();
    errors.extend(manifest.missing_on(&caps));
    if !errors.is_empty() {
        return Err(DomainError::with_detail(
            DomainErrorCode::EndpointUnavailable,
            "这份清单装不到这台机器上",
            json!({ "errors": serde_json::to_value(&errors).expect("CompositionError 是数据") }),
        )
        .into());
    }

    // 闸门③：要连云端却没配密钥。**不带任何 key 内容**，只指到界面。
    if let Some(session) = &manifest.session {
        if ledger.session_config(endpoint).api_key.is_empty() {
            return Err(DomainError::with_detail(
                DomainErrorCode::MissingApiKey,
                "这个 provider 还没配 API 密钥",
                json!({
                    "provider": session.provider.as_id(),
                    "hint": "在设置里填一次密钥（密钥不进清单，也不进任何工具结果）",
                }),
            )
            .into());
        }
    }

    // 已经在跑：幂等返回现有 handle（对齐芯的"已经在跑就什么也不做"）。
    if ledger.pipeline_state(endpoint).is_running() {
        let handle = sessions.handle_for(endpoint);
        let state = ledger.pipeline_state(endpoint);
        return Ok(opened(&handle, endpoint, state, &manifest, 0));
    }

    let started_at = ledger.now_ms();
    ledger.start(endpoint);
    // 账本没接下这次启动（状态还是 Idle）：如实回 `start_failed`，不要干等一个上限。
    if ledger.pipeline_state(endpoint) == PipelineState::Idle {
        return Err(DomainError::with_detail(
            DomainErrorCode::StartFailed,
            "账本没有接下这次启动",
            json!({
                "reason": ledger
                    .pipeline_error(endpoint)
                    .unwrap_or_else(|| "账本拒绝了这次启动（见提示）".to_string()),
            }),
        )
        .into());
    }

    loop {
        let state = ledger.pipeline_state(endpoint);
        match state {
            PipelineState::Ready | PipelineState::Active => {
                let handle = sessions.handle_for(endpoint);
                let waited = ledger.now_ms().saturating_sub(started_at);
                return Ok(opened(&handle, endpoint, state, &manifest, waited));
            }
            PipelineState::Failed => {
                return Err(DomainError::with_detail(
                    DomainErrorCode::StartFailed,
                    "会话起来了但进了 Failed",
                    json!({ "reason": ledger.pipeline_error(endpoint).unwrap_or_default() }),
                )
                .into());
            }
            // 还在起（Starting / Reconnecting）——继续等。
            PipelineState::Idle | PipelineState::Starting | PipelineState::Reconnecting => {}
        }

        let waited = ledger.now_ms().saturating_sub(started_at);
        if waited >= u64::from(wait_ready_ms) {
            // **不留孤儿**：超时先把这条腿停掉（走握手，等它真收摊）。
            ledger.stop(endpoint);
            return Err(DomainError::with_detail(
                DomainErrorCode::StartTimeout,
                "等到上限还没就绪（已停掉，不留孤儿）",
                json!({ "waited_ms": waited }),
            )
            .into());
        }
        std::thread::sleep(READY_POLL);
    }
}

/// `session_close`：停掉一个控制面会话。**不查任何授权位**（关麦永远不该被挡住）。
pub fn close(
    ledger: &dyn Ledger,
    sessions: &mut Sessions,
    session: &str,
) -> Result<Value, CallFailure> {
    if let Some(endpoint) = sessions.forget(session) {
        ledger.stop(endpoint);
        return Ok(json!({
            "session": session,
            "endpoint": endpoint.as_str(),
            "state": "idle",
            "stopped": true,
        }));
    }
    if let Some(endpoint) = sessions.was_closed(session) {
        return Ok(json!({
            "session": session,
            "endpoint": endpoint.as_str(),
            "state": "idle",
            "stopped": false,
        }));
    }
    Err(DomainError::with_detail(
        DomainErrorCode::UnknownSession,
        "handle 不认识",
        json!({ "hint": "handle 只在本进程存活期内有效：进程重启过，或这个 handle 从来没签发过" }),
    )
    .into())
}

/// `session_open` 的成功结果。
fn opened(
    session: &str,
    endpoint: EndpointId,
    state: PipelineState,
    manifest: &vox_core::composition::Composition,
    wait_ms: u64,
) -> Value {
    json!({
        "session": session,
        "endpoint": endpoint.as_str(),
        "state": serde_json::to_value(state).expect("PipelineState 是标量枚举"),
        "transcript": format!("vox://session/{session}/transcript"),
        "manifest": endpoints::wire(manifest),
        "wait_ms": wait_ms,
    })
}

/// 授权位对应的设置开关（`permission_denied` 的 `hint`：一句话，指到具体开关）。
fn hint_of(permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => "在设置 →「Agent 控制面」里打开「允许麦克风」",
        Permission::SystemAudio => "在设置 →「Agent 控制面」里打开「允许系统音频」",
        Permission::AudibleOutput => "在设置 →「Agent 控制面」里打开「允许出声」",
    }
}

/// `prefix` + `bytes` 字节 `getrandom` 的 hex（token 与 handle 的随机部分都走这里）。
fn mint(prefix: &str, bytes: usize) -> String {
    use std::fmt::Write;

    let mut buffer = [0u8; 16];
    let raw = &mut buffer[..bytes];
    getrandom::fill(raw).expect("取随机数失败");
    let mut text = String::with_capacity(prefix.len() + bytes * 2);
    text.push_str(prefix);
    for byte in raw {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// 锁中毒不该让整个控制面趴下：被毒到的只有"谁来读这点字幕状态"这点信息，接着用就是
/// （与 `transport/http.rs` 的同名小工具同一条口径）。
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 控制面的**默认后端**：基于 vox-core 账本（[`Ledger`] 的唯一真实现就是 `Runtime`）。
///
/// 装配层可以直接用它（`LedgerBackend::new(runtime)`，再 `serve(options, Some(Box::new(...)))`），
/// 也可以自己实现 [`ControlBackend`] 去包装它——本 crate 不假设宿主怎么注入。
/// `voxctl` 的 `serve` 不注入后端：它是纯协议面的调试入口，那时 `tools/call` 如实回 `-32603`。
pub struct LedgerBackend<L: Ledger, G: Grants> {
    ledger: L,
    grants: G,
    tokens: Tokens,
    sessions: Sessions,
    /// 字幕变更检测器：事件监听器（写 `confirmed`/`done`）与 ticker（比指纹）共用一份。
    transcripts: Arc<Mutex<Transcripts>>,
}

impl<L: Ledger, G: Grants> LedgerBackend<L, G> {
    /// `grants` 由宿主注入：**真实现就是 `Runtime` 自己**——
    /// `LedgerBackend::new(runtime.clone(), runtime)`，授权位现读 `Settings.control.allow_*`
    /// （`impl Grants for Runtime`）。[`crate::ledger::Denied`] 留给用例与"控制面整体关掉"的场景。
    ///
    /// 这里**顺手订一条事件监听器**（资源面的事件口）：它只碰 [`Transcripts`] 那一份数据，
    /// 不读账本、不回调别人——芯可能在持有状态锁时调它，见 `transcript.rs` 的锁序说明。
    pub fn new(ledger: L, grants: G) -> Self {
        let transcripts = Arc::new(Mutex::new(Transcripts::default()));
        let listener: Listener = {
            let transcripts = Arc::clone(&transcripts);
            Arc::new(move |event| match event {
                Event::SubtitleDelta {
                    track,
                    confirmed,
                    done,
                    ..
                } => lock(&transcripts).note(*track, confirmed.as_deref(), *done),
                Event::SubtitleCleared { track } => lock(&transcripts).cleared(*track),
                _ => {}
            })
        };
        ledger.add_listener(listener);

        Self {
            ledger,
            grants,
            tokens: Tokens::default(),
            sessions: Sessions::default(),
            transcripts,
        }
    }
}

impl<L: Ledger, G: Grants> ControlBackend for LedgerBackend<L, G> {
    fn list_endpoints(&mut self) -> Result<Value, CallFailure> {
        endpoints::list(&self.ledger)
    }

    fn describe_endpoint(&mut self, endpoint: EndpointId) -> Result<Value, CallFailure> {
        endpoints::describe(&self.ledger, &self.grants, endpoint)
    }

    fn compose_endpoint(
        &mut self,
        endpoint: EndpointId,
        composition: Value,
        apply: bool,
        token: Option<String>,
    ) -> Result<Value, CallFailure> {
        endpoints::compose(
            &self.ledger,
            &self.grants,
            &mut self.tokens,
            endpoint,
            composition,
            apply,
            token,
        )
    }

    fn session_open(
        &mut self,
        endpoint: EndpointId,
        wait_ready_ms: u32,
    ) -> Result<Value, CallFailure> {
        open(
            &self.ledger,
            &self.grants,
            &mut self.sessions,
            endpoint,
            wait_ready_ms,
        )
    }

    fn session_close(&mut self, session: &str) -> Result<Value, CallFailure> {
        close(&self.ledger, &mut self.sessions, session)
    }

    fn list_resources(&mut self) -> Value {
        Value::Array(
            self.sessions
                .live()
                .map(|(endpoint, handle)| resources::entry(endpoint, handle))
                .collect(),
        )
    }

    fn read_resource(&mut self, uri: &str) -> Option<Value> {
        let handle = resources::handle_of(uri)?;
        let endpoint = self.sessions.endpoint_of(handle)?;

        // **先读账本，再锁检测器**（锁序见 `transcript.rs`）。
        let now_ms = self.ledger.now_ms();
        let notify_ms = self.ledger.settings().control.transcript_notify_ms;
        let text = self.ledger.subtitle_text(endpoint);
        let state = self.ledger.pipeline_state(endpoint);

        let mut transcripts = lock(&self.transcripts);
        let track = endpoints::pipeline(endpoint).track();
        // 读也是一次观察：连着读两次、中间说了句话 → `revision` 变大（验收 §4-15）。
        // 订阅流那边不受影响——它比的是**自己发到哪个 revision 的水位**，不是"revision 变没变"
        // （见 `mcp/subscriptions.rs`）。
        transcripts.observe(handle, track, &text, state, now_ms);

        let snapshot = resources::Snapshot {
            handle,
            endpoint,
            state,
            revision: transcripts.revision(handle),
            notify_ms,
            text: &text,
            confirmed: transcripts.confirmed(handle),
            last_delta_done: transcripts.last_delta_done(handle),
            updated_at_ms: transcripts.updated_at_ms(handle),
        };
        Some(resources::contents(handle, &snapshot.to_json()))
    }

    /// 总闸就是账本里那一格（`Settings.control.enabled`）——**现读**，不缓存：用户在设置里一关，
    /// 传输面的下一个 tick 就把在册的订阅流收掉。这里不另存一份"控制面开着没"的状态，
    /// 与 [`Grants for Runtime`](crate::ledger::Grants) 读的是同一格。
    fn control_enabled(&mut self) -> bool {
        self.ledger.settings().control.enabled
    }

    fn poll_resources(&mut self) -> ResourceTick {
        let now_ms = self.ledger.now_ms();
        let notify_ms = self.ledger.settings().control.transcript_notify_ms;
        let live: Vec<(EndpointId, String)> = self
            .sessions
            .live()
            .map(|(endpoint, handle)| (endpoint, handle.to_string()))
            .collect();

        // **先读账本，再锁检测器**：绝不在持有检测器锁时调账本（芯的事件回调可能在持有状态锁时
        // 进来拿检测器的锁，反着来就是死锁）。
        let readings: Vec<(String, PipelineState, String)> = live
            .iter()
            .map(|(endpoint, handle)| {
                (
                    handle.clone(),
                    self.ledger.pipeline_state(*endpoint),
                    self.ledger.subtitle_text(*endpoint),
                )
            })
            .collect();

        let mut transcripts = lock(&self.transcripts);
        transcripts.live(
            &live
                .iter()
                .map(|(_, handle)| handle.as_str())
                .collect::<Vec<_>>(),
        );

        let resources = live
            .iter()
            .zip(readings.iter())
            .map(|((endpoint, handle), (_, state, text))| {
                let track = endpoints::pipeline(*endpoint).track();
                transcripts.observe(handle, track, text, *state, now_ms);
                ResourceState {
                    uri: resources::uri(handle),
                    revision: transcripts.revision(handle),
                }
            })
            .collect();

        ResourceTick {
            notify_ms,
            list_revision: transcripts.list_revision(),
            resources,
        }
    }
}
