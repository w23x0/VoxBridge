//! 动作清单：**唯一真源**。
//!
//! 这一份是**数据**，不是代码分支：工具名、入/出参 JSON Schema（2020-12 文本常量）、需要的
//! 用户授权位、是否写配置、是否幂等、是不是长任务，全写在这张表里。`tools/list` 从它生成，
//! `tools/call` 的参数校验按它做，CLI 的子命令也从它生成——**不写第二份 schema**。
//! 改一个工具 = 改这张表 + 改 `handlers.rs` 里的一格穷尽 `match`。
//!
//! 规矩（`docs/plans/S1-AGENT-FACE.md` §2.1 + `.omp/agents/agent-face-dev.md`）：
//!
//! - 没有"万能 execute"后门：每个动作有自己的入参 schema、输出形状与失败语义；
//! - 输入是**全量**的，不许 patch（清单必须能原样序列化回读）；
//! - 唯一不手写的一格是 `compose_endpoint` 的 `composition`：形状由清单类型（S0 的
//!   `Composition`）定，schema 文本的交换点是 `composition_schema!` 那一个宏——它展开的是
//!   `build.rs` 在构建期用 `schema_for!(Composition)` 打出来的**字面量**（怎么变成字面量、
//!   内部 `$ref` 怎么改，写在那个宏与 `build.rs` 的文档注释里）。`json-schema` 关掉
//!   （`--no-default-features`）时它走另一条分支：如实放宽成任意对象，`$comment` 里写明
//!   "未开启 feature"，绝不假装是类型生成的形状；
//! - `annotations` 全部发，但**服务端自己不拿它当安全依据**（规范：客户端 MUST 视为不可信）；
//!   授权只看 [`Action::permissions`] 与 [`Action::writes_config`]——后者真的在写门上
//!   （`endpoints.rs::compose` 的第 4 步读的就是这一格，不是硬编码"哪个动作要授权"）。

use serde::Deserialize;
use serde_json::{json, Value};

/// 端点 id。现状就是两条写死的流水线（对接 `vox-core` 的 `Pipeline::{Speak, Listen}`），
/// 所以只有两个取值；"这一份清单长什么样"的投影（`Settings` → 清单）住在 `endpoints.rs`，
/// 这里只留 id 本身。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointId {
    Speak,
    Listen,
}

impl EndpointId {
    /// 全部取值：schema 文本里的 `enum` 与它必须一致（`tests/protocol.rs` 钉住）。
    pub const ALL: &'static [EndpointId] = &[EndpointId::Speak, EndpointId::Listen];

    pub const fn as_str(self) -> &'static str {
        match self {
            EndpointId::Speak => "speak",
            EndpointId::Listen => "listen",
        }
    }
}

/// 内部动作身份。分派（[`crate::handlers`]）用它，**不参与线路**；工具名住在 [`Action::name`]。
///
/// `Ord` 只为一条不变量服务：`ACTIONS` 里每一行的 `id` 排序后必须恰好等于 [`ActionId::ALL`]
/// ——它证明"表覆盖了每个动作、且 id ↔ 工具名配对没错"（穷尽 `match` 证不了配对）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActionId {
    ListEndpoints,
    DescribeEndpoint,
    ComposeEndpoint,
    SessionOpen,
    SessionClose,
}

impl ActionId {
    /// 全部动作：用来钉住"表覆盖了每一个动作，不多不少"。
    pub const ALL: &'static [ActionId] = &[
        ActionId::ListEndpoints,
        ActionId::DescribeEndpoint,
        ActionId::ComposeEndpoint,
        ActionId::SessionOpen,
        ActionId::SessionClose,
    ];
}

/// 需要"用户授权位"的能力。
///
/// **不是**能力位：能力位回答"本机有没有"（S0 的 `HostFacts`，Linux 上麦克风要 PipeWire、
/// Windows 上进程环回要 build ≥ 20348），这里回答"用户让不让"（`Settings.control.allow_*`，
/// 默认全 false，必须用户主动打开——`impl Grants for Runtime` 读的就是那几格）。前两个与 S0 的
/// 能力位一一对应（`mic` / `program_tap`），第三个**没有**对应的能力位（任何档位都能出声），
/// 但一样有授权位（`allow_audible_output`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Microphone,
    SystemAudio,
    AudibleOutput,
}

impl Permission {
    /// 线路上的名字：`describe_endpoint.permissions[].permission` 与
    /// `permission_denied.detail.permission` 都用它。与 `DESCRIBE_ENDPOINT_OUT` 里那个 `enum`
    /// **必须一致**（`tests/protocol.rs` 钉着）。
    pub const fn as_str(self) -> &'static str {
        match self {
            Permission::Microphone => "microphone",
            Permission::SystemAudio => "system_audio",
            Permission::AudibleOutput => "audible_output",
        }
    }

    /// 全部取值。给"名字 ↔ schema 的 `enum` 对得上"这条不变量用。
    pub const ALL: &'static [Permission] = &[
        Permission::Microphone,
        Permission::SystemAudio,
        Permission::AudibleOutput,
    ];
}

/// 一个动作要用户开的授权位。**空 = 不碰设备**。
///
/// `session_open` 两组不同：`speak` 要麦克风、`listen` 抓程序音，两者都要能出声。
/// 写成并集就是"打开 listen 却要麦克风权限"的谎——所以按端点分开写。
#[derive(Debug, Clone, Copy)]
pub enum Permissions {
    /// 与端点无关的一组。
    Any(&'static [Permission]),
    /// 按端点分（只有 `session_open`）。
    ByEndpoint {
        speak: &'static [Permission],
        listen: &'static [Permission],
    },
}

/// 没给端点时的兜底。**宁严不松**：`ByEndpoint` 只出现在 `session_open`（它的入参里必有端点），
/// 走到这一格说明调用方漏了参数，按并集要权限，绝不静默放行。
const ALL_DEVICE_PERMISSIONS: &[Permission] = &[
    Permission::Microphone,
    Permission::SystemAudio,
    Permission::AudibleOutput,
];

impl Permissions {
    /// 一次调用实际要的用户授权位。没有端点参数的动作传 `None`。
    pub const fn for_endpoint(self, endpoint: Option<EndpointId>) -> &'static [Permission] {
        match self {
            Permissions::Any(set) => set,
            Permissions::ByEndpoint { speak, listen } => match endpoint {
                Some(EndpointId::Speak) => speak,
                Some(EndpointId::Listen) => listen,
                None => ALL_DEVICE_PERMISSIONS,
            },
        }
    }
}

/// 动作能跑多久。v1 只有一个取值。
///
/// **会话不是长任务**：`session_open` 开出来的东西是"持续存在"的，不是"会跑完"的活，
/// 用 handle + 资源订阅表达（`docs/plans/S1-AGENT-FACE.md` §2.3.4）。做成枚举而不是 `bool`，
/// 是为了以后接 Tasks 扩展（`io.modelcontextprotocol/tasks`）时是一次显式的类型扩展，
/// 而不是改一个布尔的语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duration {
    Immediate,
}

/// 领域失败码（v1 全集）：`isError` 结果里 `structuredContent.error.code`。
///
/// 用 snake_case **字符串**而不是数字：规范说新错误码 SHOULD 分配在 JSON-RPC 保留区间之外
/// （`basic/index#error-codes`），而我们一个数字都不自造。每条的 `data`/`detail` 形状见设计稿 §2.1.2。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainErrorCode {
    /// 用户位没开，或 OS 没授权。`detail`: `{ permission, gate: "vox_user" | "os", hint }`。
    PermissionDenied,
    /// 本机做不了这个端点（没有进程环回、没有虚拟麦…；或这份清单不是本机档位的）。
    /// `detail`: `{ errors: [CompositionError…] }`——**直接透传** S0 `Composition::missing_on()`
    /// 的结果（`kind` 判别变体：缺能力位 / `host_mismatch`），实现侧不自己拼 reason/platform 字符串。
    EndpointUnavailable,
    /// 清单里改了 `editable` 之外的格。`detail`: `{ path, why, expected? }`。
    UnsupportedField,
    /// `control.allow_config_write` 没开。`detail`: `{ hint }`。
    ConfigWriteDenied,
    /// compose token 过期 / 不匹配 / 重放。`detail`: `{ changed, expires_in_ms }`。
    ComposeTokenStale,
    /// 该 provider 没配密钥（**不带任何 key 内容**）。`detail`: `{ provider, hint }`。
    MissingApiKey,
    /// 会话起来了但进了 `Failed`。`detail`: `{ reason }`。
    StartFailed,
    /// 等到 `wait_ready_ms` 还没 Ready（**已自动停掉，不留孤儿**）。`detail`: `{ waited_ms }`。
    StartTimeout,
    /// handle 不认识（进程重启过 / 已关过）。`detail`: `{ hint }`。
    UnknownSession,
}

impl DomainErrorCode {
    /// 全部取值：用来钉住"码表不重不漏"（客户端按字符串分支，撞码就是坏契约）。
    pub const ALL: &'static [DomainErrorCode] = &[
        DomainErrorCode::PermissionDenied,
        DomainErrorCode::EndpointUnavailable,
        DomainErrorCode::UnsupportedField,
        DomainErrorCode::ConfigWriteDenied,
        DomainErrorCode::ComposeTokenStale,
        DomainErrorCode::MissingApiKey,
        DomainErrorCode::StartFailed,
        DomainErrorCode::StartTimeout,
        DomainErrorCode::UnknownSession,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            DomainErrorCode::PermissionDenied => "permission_denied",
            DomainErrorCode::EndpointUnavailable => "endpoint_unavailable",
            DomainErrorCode::UnsupportedField => "unsupported_field",
            DomainErrorCode::ConfigWriteDenied => "config_write_denied",
            DomainErrorCode::ComposeTokenStale => "compose_token_stale",
            DomainErrorCode::MissingApiKey => "missing_api_key",
            DomainErrorCode::StartFailed => "start_failed",
            DomainErrorCode::StartTimeout => "start_timeout",
            DomainErrorCode::UnknownSession => "unknown_session",
        }
    }
}

/// 一次领域失败：`resultType:"complete"` + `isError:true` + `structuredContent.error`。
///
/// 领域失败**故意不走** JSON-RPC error：规范把工具设计成模型可控的，模型要能看见失败并改参数；
/// JSON-RPC error 是给宿主/传输层看的（`server/tools` 的 Error Handling）。
#[derive(Debug, Clone)]
pub struct DomainError {
    pub code: DomainErrorCode,
    pub message: String,
    pub detail: Option<Value>,
}

impl DomainError {
    pub fn new(code: DomainErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(code: DomainErrorCode, message: impl Into<String>, detail: Value) -> Self {
        Self {
            code,
            message: message.into(),
            detail: Some(detail),
        }
    }

    /// `structuredContent.error` 的形状。
    pub fn to_json(&self) -> Value {
        let mut error = json!({ "code": self.code.as_str(), "message": self.message });
        if let Some(detail) = &self.detail {
            error["detail"] = detail.clone();
        }
        error
    }
}

/// `compose_endpoint` 里 `composition` 那一格（以及每份返回清单的输出 schema 的 `$defs` 里那一格）
/// 的 schema 文本。[`output_defs_head!`] 把它接在 `"composition": ` 后面，所以它必须是**字面量**。
///
/// **它不是手写的**：形状由清单类型 `vox_core::composition::Composition` 定（S0 的约束：清单的
/// serde 形态就是这一格，`Composition::validate()` 就是参数校验）。链路是：
///
/// ```text
/// build.rs（构建期）→ $OUT_DIR/composition.schema.json → include_str! → 本宏 → concat! → 线上文本
/// ```
///
/// `build.rs` 打的是 `schema_for!(Composition)` 的**当前**输出，所以这一格永远与类型同源，
/// 不会漂；它顺手做两件投影（见 `build.rs` 的 `project` / `rescope_refs`）：
///
/// 1. 去掉嵌套的 `$schema`——schema 0.8 产出的是一份**独立文档**（draft-07），而本文件是
///    2020-12、这一格只是子 schema：只有根节点能带 `$schema`；
/// 2. 内部引用改写成 `#/$defs/composition/definitions/…`——0.8 的子定义挂在自己的 `definitions`
///    下、引用写 `#/definitions/…`，原样塞进来会解析到**外层文档的根**（那里没有 `definitions`），
///    等于发一份解不开的坏 schema。
///
/// `schemars` 不在本 crate 的**直接依赖表**（`Cargo.toml` 的 `[dependencies]`）里：生成发生在
/// 构建期（`[build-dependencies]`），跑起来的进程只读那个文本常量。**别读成"整棵树都没有"**：
/// `json-schema`（默认开）会经 `vox-core/json-schema` 把 schemars 带进运行期依赖树
/// （`cargo tree -p vox-mcp -e normal | grep schemars` 有命中）；`--no-default-features` 关掉
/// 这条路径时，运行期树里才一份没有——那时链接的是下面那条**放宽的占位**分支（不读生成物、
/// 也不广告字段级的形状）。
///
/// 两条路径的分界只有这一个宏：`tests/protocol.rs` 各有一条用例（开着时逐格核对生成物与
/// `schema_for!(Composition)` 同源，关着时核对占位如实说"未开启"）。
/// 写作宏而不是 `const`：schema 是**文本常量**，同一份文本要塞进 `concat!` 里的多处
/// （每个用到清单的输出 schema 的 `$defs`），而 `concat!` 只吃字面量——宏展开成的字面量正好。
#[cfg(feature = "json-schema")]
macro_rules! composition_schema {
    () => {
        include_str!(concat!(env!("OUT_DIR"), "/composition.schema.json"))
    };
}

/// `json-schema` 没开时的 `composition` 那一格：**如实放宽成"任意对象"**（清单的 serde 形态本来
/// 就是对象）。这不是"将来会接"的承诺——开那个 feature（本 crate 默认开，`--no-default-features`
/// 关掉）这一格就是生成物，见上面那条 `#[cfg(feature = "json-schema")]` 的定义。
///
/// 代价是给模型的字段级提示少一份描述，**不是安全洞**：服务端**始终**用
/// `Composition::validate()`（+ `missing_on()`）兜底，从不把 schema 当权威（设计稿 §5.2 第 6 条）。
#[cfg(not(feature = "json-schema"))]
macro_rules! composition_schema {
    () => {
        r##"{ "type": "object", "$comment": "本格未开启 json-schema feature（vox-mcp 默认开；关掉是 --no-default-features）：清单的形状由 vox-core::composition::Composition 的类型生成，这里如实放宽成任意对象——没有字段级提示，服务端仍按 Composition::validate() 校验。" }"##
    };
}

/// 输出 schema 的 `$defs` 头：错误形状（[`DomainError`]）+ 清单形状。
macro_rules! output_defs_head {
    () => {
        r##"  "$defs": {
    "error": {
      "type": "object",
      "description": "领域失败（isError:true 时 structuredContent 只有这一格）",
      "properties": {
        "code": { "type": "string", "description": "snake_case 领域错误码，见 DomainErrorCode" },
        "message": { "type": "string" },
        "detail": {}
      },
      "required": ["code", "message"]
    },
    "composition": "##
    };
}

/// 输出 schema 的 `$defs` 尾：关掉 `$defs` 与根对象。
macro_rules! output_defs_close {
    () => {
        "\n  }\n}"
    };
}

/// 一个动作的完整定义。
pub struct Action {
    /// 内部身份；分派用它（[`crate::handlers::invoke`] 穷尽 `match`）。
    pub id: ActionId,
    /// MCP 工具名（`tools/list` 的 `name`，客户端的 `tools/call` 用它）。
    pub name: &'static str,
    /// 人读标题（`tools/list` 的 `title`）。
    pub title: &'static str,
    /// 给模型看的说明（含"先调谁后调谁"）。
    pub description: &'static str,
    /// `inputSchema`（JSON Schema 2020-12 文本）。**无参数的工具也要是合法 schema 对象**。
    pub input_schema: &'static str,
    /// `outputSchema`，对应 `structuredContent`。
    pub output_schema: &'static str,
    /// 要用户开的授权位（空 = 不碰设备）。
    pub permissions: Permissions,
    /// true = 会改用户配置 → 要 `control.allow_config_write` 位。
    pub writes_config: bool,
    /// true = 不改环境（`tools/list` 的 `readOnlyHint`）。注意 `session_close` **不是**只读的：
    /// 它停东西，只是"不改配置"——所以它不靠 [`Action::writes_config`] 反推。
    pub read_only: bool,
    pub idempotent: bool,
    pub long_running: Duration,
}

impl Action {
    /// `tools/list` 里的 `annotations`。
    ///
    /// 全部发；但**服务端自己不拿它当安全依据**（规范：客户端 MUST 把注解视为不可信）。
    /// 其中 `destructiveHint` 就取 [`Action::writes_config`]：只有改用户配置才算"破坏性更新"，
    /// 起停设备不是。`openWorldHint` 恒 false：5 个动作只碰本机设备/配置，不接触外部实体。
    pub fn annotations(&self) -> Value {
        json!({
            "readOnlyHint": self.read_only,
            "destructiveHint": self.writes_config,
            "idempotentHint": self.idempotent,
            "openWorldHint": false,
        })
    }

    /// CLI 子命令名：工具名的 kebab-case（`list_endpoints` → `list-endpoints`）。
    /// 工具名只允许小写字母与下划线（`server/tools#tool-names` 的字符集），所以这个替换是等价变换。
    pub fn cli_command(&self) -> String {
        self.name.replace('_', "-")
    }
}

/// `session_open` 的 `wait_ready_ms` 上限与默认值（schema 文本与手写校验共用一份）。
pub const DEFAULT_WAIT_READY_MS: u32 = 5_000;
pub const MAX_WAIT_READY_MS: u32 = 10_000;

/// `compose_endpoint` 里 `token` 的存活时间（秒→毫秒），与设计稿 §2.1.3-③ 的 "TTL 120 s" 一致。
pub const COMPOSE_TOKEN_TTL_MS: u64 = 120_000;

const LIST_ENDPOINTS_IN: &str = r##"{ "type": "object", "additionalProperties": false }"##;

const LIST_ENDPOINTS_OUT: &str = concat!(
    r##"{
  "type": "object",
  "properties": {
    "device": {
      "type": "object",
      "properties": {
        "tier": { "type": "string", "description": "本机宿主档位：windows / linux_desktop / android / linux_headless（= CapabilityReport.tier；清单里的 host 是同一档位）" },
        "control": { "type": "array", "items": { "type": "string" }, "description": "这台设备上控制面开了哪些通道" }
      },
      "required": ["tier", "control"]
    },
    "endpoints": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "id": { "type": "string", "enum": ["speak", "listen"] },
          "title": { "type": "string" },
          "available": { "type": "boolean", "description": "本机能不能开（能力位说了算）" },
          "running": { "type": "boolean" },
          "summary": { "type": "string", "description": "一句话：这条腿做什么" },
          "unavailable_reason": { "type": "string", "description": "available=false 时给出原因" }
        },
        "required": ["id", "title", "available", "running", "summary"]
      }
    },
    "error": { "$ref": "#/$defs/error" }
  },
  "anyOf": [{ "required": ["device", "endpoints"] }, { "required": ["error"] }],
"##,
    output_defs_head!(),
    composition_schema!(),
    output_defs_close!(),
);

const DESCRIBE_ENDPOINT_IN: &str = r##"{
  "type": "object",
  "properties": {
    "endpoint": { "type": "string", "enum": ["speak", "listen"], "description": "list_endpoints 返回的 id" }
  },
  "required": ["endpoint"],
  "additionalProperties": false
}"##;

const DESCRIBE_ENDPOINT_OUT: &str = concat!(
    r##"{
  "type": "object",
  "properties": {
    "endpoint": { "type": "string", "enum": ["speak", "listen"] },
    "title": { "type": "string" },
    "available": { "type": "boolean" },
    "manifest": { "$ref": "#/$defs/composition", "description": "S0 的组合清单，逐字（含 schema_version）；S1 不解析、不改写" },
    "capabilities": { "type": "object", "description": "S0 的 CapabilityReport 同构：tier 档位 + host 位表 + provider 位；S1 不另定义能力位" },
    "permissions": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "permission": { "type": "string", "enum": ["microphone", "system_audio", "audible_output"] },
          "user_granted": { "type": "boolean" },
          "os_granted": { "type": "boolean" }
        },
        "required": ["permission", "user_granted", "os_granted"]
      }
    },
    "editable": { "type": "array", "items": { "type": "string" }, "description": "这份清单里哪几格可以改（清单路径）" },
    "running": { "type": "boolean" },
    "notes": { "type": "array", "items": { "type": "string" } },
    "error": { "$ref": "#/$defs/error" }
  },
  "anyOf": [
    { "required": ["endpoint", "title", "available", "manifest", "capabilities", "permissions", "editable", "running"] },
    { "required": ["error"] }
  ],
"##,
    output_defs_head!(),
    composition_schema!(),
    output_defs_close!(),
);

const COMPOSE_ENDPOINT_IN: &str = concat!(
    r##"{
  "type": "object",
  "properties": {
    "endpoint": { "type": "string", "enum": ["speak", "listen"], "description": "list_endpoints 返回的 id" },
    "composition": { "$ref": "#/$defs/composition", "description": "完整清单。先 describe_endpoint 拿当前那份，改你想改的格，再整份发回来" },
    "apply": { "type": "boolean", "description": "false = 只算差异（dry-run）；true = 落进账本，必须带 token。**必填**，没有默认值：默认值是危险的" },
    "token": { "type": "string", "description": "apply=true 时必填：上一次 dry-run 返回的 token" }
  },
  "required": ["endpoint", "composition", "apply"],
  "if": { "properties": { "apply": { "const": true } }, "required": ["apply"] },
  "then": { "required": ["token"] },
  "additionalProperties": false,
  "$defs": { "composition": "##,
    composition_schema!(),
    r##" }
}"##
);

const COMPOSE_ENDPOINT_OUT: &str = concat!(
    r##"{
  "type": "object",
  "properties": {
    "endpoint": { "type": "string", "enum": ["speak", "listen"] },
    "applied": { "type": "boolean", "description": "true = 已经落进账本；false = 只是 dry-run" },
    "changed": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "path": { "type": "string", "description": "清单路径，如 session.params.target_language" },
          "from": {},
          "to": {}
        },
        "required": ["path"]
      }
    },
    "manifest": { "$ref": "#/$defs/composition", "description": "应用后的完整清单（dry-run 时也要给，方便核对）" },
    "token": { "type": "string", "description": "dry-run 才有：一次性 token，apply 时带回来" },
    "expires_in_ms": { "type": "integer", "minimum": 0 },
    "notes": { "type": "array", "items": { "type": "string" } },
    "error": { "$ref": "#/$defs/error" }
  },
  "anyOf": [
    { "required": ["endpoint", "applied", "changed", "manifest"] },
    { "required": ["error"] }
  ],
"##,
    output_defs_head!(),
    composition_schema!(),
    output_defs_close!(),
);

const SESSION_OPEN_IN: &str = r##"{
  "type": "object",
  "properties": {
    "endpoint": { "type": "string", "enum": ["speak", "listen"] },
    "wait_ready_ms": { "type": "integer", "minimum": 0, "maximum": 10000, "default": 5000,
                       "description": "等到 Ready/Active/Failed 的上限；超时会自动停掉，不留孤儿" }
  },
  "required": ["endpoint"],
  "additionalProperties": false
}"##;

const SESSION_OPEN_OUT: &str = concat!(
    r##"{
  "type": "object",
  "properties": {
    "session": { "type": "string", "description": "服务端签发的 handle；后续每个调用都要显式带上（协议无 session）" },
    "endpoint": { "type": "string", "enum": ["speak", "listen"] },
    "state": { "type": "string", "description": "ready / active" },
    "transcript": { "type": "string", "description": "字幕资源 URI：vox://session/<handle>/transcript" },
    "manifest": { "$ref": "#/$defs/composition", "description": "生效中的清单" },
    "wait_ms": { "type": "integer", "minimum": 0 },
    "error": { "$ref": "#/$defs/error" }
  },
  "anyOf": [
    { "required": ["session", "endpoint", "state", "transcript", "manifest", "wait_ms"] },
    { "required": ["error"] }
  ],
"##,
    output_defs_head!(),
    composition_schema!(),
    output_defs_close!(),
);

const SESSION_CLOSE_IN: &str = r##"{
  "type": "object",
  "properties": {
    "session": { "type": "string", "description": "session_open 返回的 handle" }
  },
  "required": ["session"],
  "additionalProperties": false
}"##;

const SESSION_CLOSE_OUT: &str = concat!(
    r##"{
  "type": "object",
  "properties": {
    "session": { "type": "string" },
    "endpoint": { "type": "string", "enum": ["speak", "listen"] },
    "state": { "type": "string", "description": "关完是 idle" },
    "stopped": { "type": "boolean", "description": "false = 这个 handle 本来就关过了（幂等）" },
    "error": { "$ref": "#/$defs/error" }
  },
  "anyOf": [
    { "required": ["session", "endpoint", "state", "stopped"] },
    { "required": ["error"] }
  ],
"##,
    output_defs_head!(),
    composition_schema!(),
    output_defs_close!(),
);

/// 动作清单。**顺序即 `tools/list` 顺序**（规范 SHOULD 确定顺序：客户端靠它缓存工具列表、
/// 也靠它吃 prompt 缓存）。第一行是"这台设备能开什么"，第二行是"这一份清单长什么样"，
/// 第三行才是写——读—改—写。
pub static ACTIONS: &[Action] = &[
    Action {
        id: ActionId::ListEndpoints,
        name: "list_endpoints",
        title: "列出端点",
        description: "列出这台设备能开的端点：id、标题、现在能不能开（available）、是否正在跑（running）、\
一句话摘要。先调它拿 id，再调 describe_endpoint 看清单；要改配置走 compose_endpoint，要开跑走 session_open。",
        input_schema: LIST_ENDPOINTS_IN,
        output_schema: LIST_ENDPOINTS_OUT,
        permissions: Permissions::Any(&[]),
        writes_config: false,
        read_only: true,
        idempotent: true,
        long_running: Duration::Immediate,
    },
    Action {
        id: ActionId::DescribeEndpoint,
        name: "describe_endpoint",
        title: "看一个端点的清单",
        description: "取一个端点的完整清单（in/ops/out/session…）、能力位、权限状态与可改的格（editable）。\
清单是只读投影：把返回的 manifest 原样发回 compose_endpoint 应当零差异。先调 list_endpoints 拿 endpoint。",
        input_schema: DESCRIBE_ENDPOINT_IN,
        output_schema: DESCRIBE_ENDPOINT_OUT,
        permissions: Permissions::Any(&[]),
        writes_config: false,
        read_only: true,
        idempotent: true,
        long_running: Duration::Immediate,
    },
    Action {
        id: ActionId::ComposeEndpoint,
        name: "compose_endpoint",
        title: "改端点清单",
        description: "读—改—写端点清单：先 describe_endpoint 拿当前那份，只改 editable 里列的那几格，再整份发回来。\
apply=false（默认）只算差异并给一个一次性 token；apply=true 必须带上同一个 token 才落进账本。\
改了 editable 之外的格会返回 unsupported_field，附 path 与 why。",
        input_schema: COMPOSE_ENDPOINT_IN,
        output_schema: COMPOSE_ENDPOINT_OUT,
        permissions: Permissions::Any(&[]),
        writes_config: true,
        read_only: false,
        idempotent: false,
        long_running: Duration::Immediate,
    },
    Action {
        id: ActionId::SessionOpen,
        name: "session_open",
        title: "开一个端点",
        description: "按当前清单把端点真的跑起来（采集、会话、字幕）。已经在跑时幂等返回现有 handle。\
handle 只在本进程存活期内有效，且必须显式传给后续每个调用（协议没有 session）。\
实时音频不走本协议：它是媒体面的事。",
        input_schema: SESSION_OPEN_IN,
        output_schema: SESSION_OPEN_OUT,
        permissions: Permissions::ByEndpoint {
            speak: &[Permission::Microphone, Permission::AudibleOutput],
            listen: &[Permission::SystemAudio, Permission::AudibleOutput],
        },
        writes_config: false,
        read_only: false,
        idempotent: true,
        long_running: Duration::Immediate,
    },
    Action {
        id: ActionId::SessionClose,
        name: "session_close",
        title: "关口端点",
        description: "停掉一个控制面会话（handle 由 session_open 返回）。幂等：已经关过的 handle 返回 stopped=false。\
不需要任何授权位——**关麦永远不该被授权位挡住**。",
        input_schema: SESSION_CLOSE_IN,
        output_schema: SESSION_CLOSE_OUT,
        permissions: Permissions::Any(&[]),
        writes_config: false,
        read_only: false,
        idempotent: true,
        long_running: Duration::Immediate,
    },
];

/// 按内部身份取一条动作定义。
///
/// 表 ↔ 枚举的配对由 `tests/protocol.rs::action_table_ids_are_unique_and_match_every_action`
/// 钉着（编译器证不了配对），所以这里可以 `expect`。
pub fn action_by_id(id: ActionId) -> &'static Action {
    ACTIONS
        .iter()
        .find(|action| action.id == id)
        .expect("每个 ActionId 都在表里")
}
