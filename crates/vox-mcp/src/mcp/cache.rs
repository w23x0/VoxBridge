//! 缓存定值表（`ttlMs` + `cacheScope`）。
//!
//! 规范（`changelog` Minor 5 + `server/utilities/caching`）：`server/discover`、`tools/list`、
//! `resources/*` 等结果**必须**带这两个提示；`ttlMs` 必须 `>= 0`。它不是保证，而是
//! "客户端在这段时间里可以不重取"的提示。

use serde_json::{Map, Value};

/// 一对缓存提示。
#[derive(Debug, Clone, Copy)]
pub struct CacheHints {
    /// 新鲜期（毫秒）。`0` = 立刻算陈旧（客户端可以每次重取）。
    pub ttl_ms: u64,
    /// `"public"` = 不含用户私有数据，可以共享；`"private"` = 只许在同一个授权上下文里复用。
    pub cache_scope: &'static str,
}

impl CacheHints {
    /// 把两个提示写进结果对象。
    pub fn apply(self, result: &mut Map<String, Value>) {
        result.insert("ttlMs".to_string(), Value::from(self.ttl_ms));
        result.insert(
            "cacheScope".to_string(),
            Value::String(self.cache_scope.to_string()),
        );
    }
}

/// 能力在一次进程存活期里不变。
pub const DISCOVER: CacheHints = CacheHints {
    ttl_ms: 3_600_000,
    cache_scope: "public",
};

/// 5 个工具编译期恒定，且**不因授权位变化而增删工具**（缺权限是调用时失败，不做"藏工具"）。
pub const TOOLS_LIST: CacheHints = CacheHints {
    ttl_ms: 3_600_000,
    cache_scope: "public",
};

/// `resources/list`：会话随时开/关，而且那是**用户私有**信息（谁在说话）。
pub const RESOURCES_LIST: CacheHints = CacheHints {
    ttl_ms: 0,
    cache_scope: "private",
};

/// `resources/read`（字幕）：**`0`** —— 不许缓存住旧字幕，客户端每次要都该重取；
/// `private` —— 转写是用户私有数据，只许在同一个授权上下文里复用（设计稿 §2.3.2）。
pub const RESOURCES_READ: CacheHints = CacheHints {
    ttl_ms: 0,
    cache_scope: "private",
};

// `tools/call` 与 `subscriptions/listen` 都不带缓存提示：规范的可缓存清单里没有它们
// （`listen` 的响应根本不是一条 result，是一条长流）。
