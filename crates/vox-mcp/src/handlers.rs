//! 动作 → 后端：`ActionId` 的**穷尽 `match`**，加上参数校验。
//!
//! 这一层是**唯一**碰账本/设备的地方，而它自己并不知道账本是什么：它只认
//! [`ControlBackend`]。实现由外壳注入（装配层的控制面胶水；缺省那份
//! [`crate::session::LedgerBackend`] 直接吃芯的 [`vox_core::runtime::Runtime`]）——
//! 「两条流水线 + `Runtime` 是唯一账本」的规矩不在本 crate 里破。
//!
//! 为什么用 `match` 而不是按名字查表：穷尽 `match` 让编译器保证"**每个 `ActionId` 都有 handler**"
//! ——加一个动作而忘了接线会编译不过，所以不写"分派覆盖了全部动作"的测试。
//!
//! 但编译器**证不了**"`ACTIONS` 表里 id ↔ 工具名配对没错"（把某一行的 id 换成别的变体照样编译）：
//! 那条由 `tests/protocol.rs` 的 `action_table_ids_are_unique_and_match_every_action` 断言。

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::actions::{
    ActionId, DomainError, EndpointId, ACTIONS, DEFAULT_WAIT_READY_MS, MAX_WAIT_READY_MS,
};
use crate::jsonrpc::{code, ErrorObject};
use crate::resources::ResourceTick;

/// 一次动作失败。**两条通道**，因为规范要求它们走不同地方（`docs/plans/S1-AGENT-FACE.md` §2.1.2）：
///
/// - 结构性不合法（清单过不了 `Composition::validate()`）是**协议错误**：模型改不了结构，
///   给宿主/传输层看更合适 → JSON-RPC `-32602` + `data.errors`；
/// - 世界不允许（没授权、设备不支持、token 过期…）是**工具执行错误**：模型看得见、改得了参数
///   → `isError:true` + `structuredContent.error`。
#[derive(Debug, Clone)]
pub enum CallFailure {
    /// 清单不满足 `Composition::validate()`。`errors` 是 S0 的 `CompositionError` 列表
    /// （已序列化：一次给全部问题，不是遇错就返回）。
    InvalidParams { message: String, errors: Value },
    /// 领域失败。
    Domain(DomainError),
}

impl CallFailure {
    /// 清单校验失败（`errors` = S0 `CompositionError` 的序列化数组）。
    pub fn composition(errors: Value) -> Self {
        CallFailure::InvalidParams {
            message: "清单不满足 Composition::validate()".to_string(),
            errors,
        }
    }
}

impl From<DomainError> for CallFailure {
    fn from(error: DomainError) -> Self {
        CallFailure::Domain(error)
    }
}

/// 控制面后端：**唯一**碰账本与设备的入口，由外壳注入。
///
/// 契约（谁实现谁负责）：
///
/// - 世界不允许 → 返回 [`DomainError`]（协议层翻成 `isError` 工具结果）；结构性不合法
///   （`Composition::validate()` 失败）→ 返回 [`CallFailure::InvalidParams`]（协议层翻成
///   `-32602` + `data.errors`）。`DomainError` 有 `From` 实现，`?` 就能用。
/// - **不要**在成功路径上编造字段：返回的对象就是工具结果的 `structuredContent`，按
///   `actions::ACTIONS` 里那份 `output_schema` 的形状给（`tests/protocol.rs` 逐格核对
///   清单与 schema 的一致性）。
/// - 用户授权位、能力位都在实现侧查（S1 §2.5.2 的三个闸门）。
/// - 实现必须是**同步、阻塞**的：本 crate 零 async，与"芯没有 async"的口径一致。
pub trait ControlBackend {
    /// `list_endpoints`：这台设备能开的端点 + 本机控制面通道（`device.tier` 是本机档位）。
    fn list_endpoints(&mut self) -> Result<Value, CallFailure>;

    /// `describe_endpoint`：一个端点的清单投影、能力位、权限状态、可改的格。
    ///
    /// 两条要守的规矩：① `editable` 是 §2.1.4 那张表的**键**（`out[playback(primary)].device`
    /// 这种写法，只到"能改哪一格"）；② `role` **跟事实走**——`virtual_mic` 位关着时
    /// `out[0].role` 就是 `speaker`，不许按设备名或按"这条腿是什么"硬编码；反写前先按当前
    /// 能力位算一遍"本机现在该是什么 role"再比，别拿上一次的值当基准。
    fn describe_endpoint(&mut self, endpoint: EndpointId) -> Result<Value, CallFailure>;

    /// `compose_endpoint`：两段式落进账本。`apply == false` 时只算差异并签发 token。
    ///
    /// **四步顺序固定**：`Composition::validate()` → `missing_on()` → diff（只认 `editable`
    /// 里的格）→ dry-run / apply。`missing_on` **必须**排在 diff 前面：反过来的话，把 `host`
    /// 改成别的档位会先撞 `unsupported_field`，`HostMismatch` 永远不可达（验收 §4-12 有这条负例）。
    fn compose_endpoint(
        &mut self,
        endpoint: EndpointId,
        composition: Value,
        apply: bool,
        token: Option<String>,
    ) -> Result<Value, CallFailure>;

    /// `session_open`：按当前清单把端点跑起来（已在跑则幂等返回现有 handle）。
    fn session_open(
        &mut self,
        endpoint: EndpointId,
        wait_ready_ms: u32,
    ) -> Result<Value, CallFailure>;

    /// `session_close`：停掉一个 handle。已关过 → 成功 + `stopped: false`。
    fn session_close(&mut self, session: &str) -> Result<Value, CallFailure>;

    /// `resources/list` 的数据面：现在**活着**的会话各自一条字幕资源（一条都没有就是空数组）。
    ///
    /// 只列控制面自己签发的 handle（`session_open` 返回的那些）——URI 里的 handle 是资源的身份，
    /// 界面自己开起来的流水线没有 handle，也就没有资源。
    fn list_resources(&mut self) -> Value;

    /// `resources/read` 的数据面：一条字幕资源的 `contents[0]`（`{uri, mimeType, text}`）。
    ///
    /// `None` = **没有这条资源**（URI 不是我们签发的形状，或那条会话已经关了）。协议层把
    /// `None` 翻成 `-32602`（规范 MUST，不是旧的 `-32002`），所以这里不返回笼统的错误。
    fn read_resource(&mut self, uri: &str) -> Option<Value>;

    /// 资源变更的轮询口：传输面的 ticker 每 `notify_ms` 调一次（**没有订阅流时不调**）。
    ///
    /// 返回值里既有"这一拍变了什么"，也有"下一拍等多久"（＝
    /// `Settings.control.transcript_notify_ms`，现读，改了下一个 tick 就生效）。ticker 只按它
    /// 决定发不发、等多久，自己不认识任何资源语义。
    fn poll_resources(&mut self) -> ResourceTick;

    /// **总闸**（`Settings.control.enabled`）现在开着没：传输面拿它回答"已经开着的订阅流要不要收掉"。
    ///
    /// 用户把控制面整体关掉之后，那条流**不许静默挂着**（S1 稿的承诺）：ticker 看到 `false` 就把
    /// 在册的流全收掉（走规范那条干净结束的路），并且不再接受新的订阅流（`mcp::listen`）。
    ///
    /// 它与 [`ControlBackend::poll_resources`] 分开是有意的：这是"服务端还要不要继续对外推"的
    /// 一条**状态**，不是资源事实，混进 `ResourceTick` 会让资源形状说谎（那个结构描述的是资源）。
    /// 实现要**现读**账本设置（不缓存），用户在设置里一关，下一个 tick 就收流、下一次 `listen`
    /// 就被拒——与 [`Grants`](crate::ledger::Grants) 那两道 fail-closed 读的是同一格。
    fn control_enabled(&mut self) -> bool;
}

/// 装箱的控制面后端。传输面**每个连接一个线程**，所以这里多要一个 `Send`：后端会被搬到
/// 连接线程上执行。[`ControlBackend`] 本身不要求 `Send`——协议层不需要它，只有线程化的
/// 传输面需要。
pub type BoxedBackend = Box<dyn ControlBackend + Send>;

/// 一次已通过入参校验的动作调用。
///
/// [`parse_call`] 造它（协议层职责：参数不合法就是 `-32602`），[`invoke`] 消费它（后端职责）。
/// 这个类型存在的意义：把"线上来的 JSON"和"后端要的类型"切开，两边的 `match` 都是穷尽的。
#[derive(Debug, Clone, PartialEq)]
pub enum ActionCall {
    ListEndpoints,
    DescribeEndpoint {
        endpoint: EndpointId,
    },
    ComposeEndpoint {
        endpoint: EndpointId,
        composition: Value,
        apply: bool,
        token: Option<String>,
    },
    SessionOpen {
        endpoint: EndpointId,
        wait_ready_ms: u32,
    },
    SessionClose {
        session: String,
    },
}

/// `tools/call` 的 `name` + `arguments` → [`ActionCall`]。
///
/// 失败一律 `-32602`（规范：参数不满足 `inputSchema` 属协议错误；HTTP 上还是 400）。
/// 本轮没有 JSON Schema 校验器（不许引新依赖），所以这里是**手写**校验，与
/// `ACTIONS` 里那份 schema 文本逐格对应：缺必填、未知键、枚举越界、数值越界、`$ref` 指向的对象类型。
/// `tests/protocol.rs` 把两边钉在一起。
pub fn parse_call(name: &str, arguments: Option<&Value>) -> Result<ActionCall, ErrorObject> {
    let Some(action) = ACTIONS.iter().find(|a| a.name == name) else {
        // 规范把"未知工具"归为协议错误（`server/tools` 的 Error Handling），不是 isError。
        return Err(ErrorObject::with_data(
            code::INVALID_PARAMS,
            format!("未知工具：{name}"),
            serde_json::json!({ "known": ACTIONS.iter().map(|a| a.name).collect::<Vec<_>>() }),
        ));
    };

    match action.id {
        ActionId::ListEndpoints => {
            // 无参数的工具：schema 是 `{"type":"object","additionalProperties":false}`，
            // 传了任何键就是未知键 → `-32602`。
            let _: NoArgs = arguments_of(arguments)?;
            Ok(ActionCall::ListEndpoints)
        }
        ActionId::DescribeEndpoint => {
            let args: DescribeArgs = arguments_of(arguments)?;
            Ok(ActionCall::DescribeEndpoint {
                endpoint: args.endpoint,
            })
        }
        ActionId::ComposeEndpoint => {
            let args: ComposeArgs = arguments_of(arguments)?;
            if !args.composition.is_object() {
                return Err(invalid_params(
                    "composition 必须是对象（清单的 serde 形态）",
                    serde_json::json!({ "path": "composition" }),
                ));
            }
            if args.apply && args.token.is_none() {
                return Err(invalid_params(
                    "apply=true 时必须带 token（上一次 dry-run 返回的那个）",
                    serde_json::json!({ "path": "token" }),
                ));
            }
            Ok(ActionCall::ComposeEndpoint {
                endpoint: args.endpoint,
                composition: args.composition,
                apply: args.apply,
                token: args.token,
            })
        }
        ActionId::SessionOpen => {
            let args: SessionOpenArgs = arguments_of(arguments)?;
            let wait_ready_ms = args.wait_ready_ms.unwrap_or(DEFAULT_WAIT_READY_MS);
            if wait_ready_ms > MAX_WAIT_READY_MS {
                return Err(invalid_params(
                    "wait_ready_ms 超出上限",
                    serde_json::json!({ "path": "wait_ready_ms", "maximum": MAX_WAIT_READY_MS, "got": wait_ready_ms }),
                ));
            }
            Ok(ActionCall::SessionOpen {
                endpoint: args.endpoint,
                wait_ready_ms,
            })
        }
        ActionId::SessionClose => {
            let args: SessionCloseArgs = arguments_of(arguments)?;
            Ok(ActionCall::SessionClose {
                session: args.session,
            })
        }
    }
}

/// 执行一个动作。★ 唯一碰账本/设备的路径：`match` 穷尽 [`ActionCall`]，每一格转发给后端。
pub fn invoke(call: ActionCall, backend: &mut dyn ControlBackend) -> Result<Value, CallFailure> {
    match call {
        ActionCall::ListEndpoints => backend.list_endpoints(),
        ActionCall::DescribeEndpoint { endpoint } => backend.describe_endpoint(endpoint),
        ActionCall::ComposeEndpoint {
            endpoint,
            composition,
            apply,
            token,
        } => backend.compose_endpoint(endpoint, composition, apply, token),
        ActionCall::SessionOpen {
            endpoint,
            wait_ready_ms,
        } => backend.session_open(endpoint, wait_ready_ms),
        ActionCall::SessionClose { session } => backend.session_close(&session),
    }
}

/// `list_endpoints` 没有参数（schema 是 `{"type":"object","additionalProperties":false}`）：
/// 用一个空结构体走同一条 `deny_unknown_fields` 路径，免得漏掉"传了参数"这一种违规。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

/// `describe_endpoint` 的入参（`deny_unknown_fields` = schema 的 `additionalProperties: false`）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeArgs {
    endpoint: EndpointId,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposeArgs {
    endpoint: EndpointId,
    composition: Value,
    apply: bool,
    /// 缺省由 `if/then` 管：`apply=true` 时 [`parse_call`] 单独报错。
    #[serde(default)]
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionOpenArgs {
    endpoint: EndpointId,
    /// 缺省 = schema 的 `default`（[`DEFAULT_WAIT_READY_MS`]）。
    #[serde(default)]
    wait_ready_ms: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionCloseArgs {
    session: String,
}

/// `arguments` 缺失 = 空对象（无参数的工具就是这么调的）；不是对象 = `-32602`。
fn arguments_of<T: DeserializeOwned>(arguments: Option<&Value>) -> Result<T, ErrorObject> {
    let value = match arguments {
        None => Value::Object(Map::new()),
        Some(value) if value.is_object() => value.clone(),
        Some(other) => {
            return Err(invalid_params(
                "arguments 必须是对象",
                serde_json::json!({ "got": other }),
            ));
        }
    };
    serde_json::from_value(value).map_err(|error| {
        // serde 的错误信息会点出是哪一格（缺必填 / 未知键 / 枚举越界），原样带给调用方。
        invalid_params(
            "参数不满足 inputSchema",
            serde_json::json!({ "reason": error.to_string() }),
        )
    })
}

fn invalid_params(message: &str, data: Value) -> ErrorObject {
    ErrorObject::with_data(code::INVALID_PARAMS, message, data)
}
