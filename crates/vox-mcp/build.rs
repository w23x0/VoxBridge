//! 生成 `compose_endpoint` 的 `composition` 那一格的 schema 文本。
//!
//! **这一格不是手写的**：它就是 `vox_core::composition::Composition` 的类型（S0 的约束：清单的
//! serde 形态**就是**这一格）。生成物写进 `$OUT_DIR/composition.schema.json`，由
//! `actions.rs::composition_schema!` 用 `include_str!` 读成**字面量**塞进 `concat!`——schema
//! 是文本常量，而 `concat!` 只吃字面量，所以生成必须发生在构建期。
//!
//! 为什么 `include_str!` 而不是检入一份生成物：那份生成物会**悄悄漂**（芯改了 `Composition`，
//! 没人跑重生成就没人知道）。这里每次都用当前的类型重打一份，漂不了。
//!
//! 三件事（前两件的"为什么"写在 `actions.rs::composition_schema!` 的文档注释里）：
//!
//! 1. 拿到 `JsonSchema` 实现：[build-dependencies] 里那份 `vox-core` 带 `json-schema` feature
//!    （build 脚本只能吃 [build-dependencies]，而 resolver v2 不把它的 feature 与正常依赖合并）。
//! 2. 把 **draft-07** 文档投影成能嵌进 `actions.rs` 那份 2020-12 输出 schema 的
//!    `$defs.composition` 的形状：去掉嵌套的 `$schema`（只有根节点能带），并把内部引用
//!    `#/definitions/X` 改写成 `#/$defs/composition/definitions/X`——不改的话引用会解析到
//!    **外层文档的根**（那里没有 `definitions`），就是一份解不开的**坏 schema**，比占位更糟。
//! 3. 写成单行 JSON（它进的是 `tools/list` 的线上文本，不给人读）。
//!
//! `json-schema` 关掉时（`--no-default-features`）**什么都不写**：那时 `composition_schema!`
//! 走放宽的占位分支，压根不读生成物（所以缺文件不会变成编译错误，而是另一条真实路径）。

use std::env;
use std::path::PathBuf;

fn main() {
    // 生成物随类型走：芯的源码一改，`vox-core`（本脚本的依赖）指纹就变，脚本会重跑。
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_JSON_SCHEMA");

    if env::var_os("CARGO_FEATURE_JSON_SCHEMA").is_none() {
        return;
    }

    let schema = serde_json::to_value(schemars::schema_for!(vox_core::composition::Composition))
        .expect("schema 是数据");
    let text = serde_json::to_string(&project(schema)).expect("schema 是数据");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo 会给 OUT_DIR"))
        .join("composition.schema.json");
    std::fs::write(&out, text).expect("要能把生成物写进 OUT_DIR");
}

/// draft-07 → 嵌进 2020-12 `$defs.composition` 的形状（见文件头第 2 条）。
fn project(mut schema: serde_json::Value) -> serde_json::Value {
    schema
        .as_object_mut()
        .expect("schemars 的产出是对象")
        .remove("$schema");
    rescope_refs(&mut schema);
    schema
}

/// 把内部引用指到输出 schema 里那一格：`#/definitions/X` → `#/$defs/composition/definitions/X`。
///
/// `$defs` 是**输出 schema**（`actions.rs` 那些常量）的根节点，而生成物的子定义挂在它自己那一层的
/// `definitions` 下（`Composition` 的子定义很多，不往 `$defs` 里平铺：那要给人家的名字加前缀，
/// 还会和输出 schema 已有的 `error` 撞名）。
fn rescope_refs(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                match (key.as_str(), child.as_str()) {
                    ("$ref", Some(reference)) => {
                        if let Some(name) = reference.strip_prefix("#/definitions/") {
                            *child = format!("#/$defs/composition/definitions/{name}").into();
                        }
                    }
                    _ => rescope_refs(child),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(rescope_refs),
        _ => {}
    }
}
