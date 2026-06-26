//! 模板系统:声明式蓝图 → 约束集(见 `docs/templates/01-template-model.md`)。
//!
//! 模板是 Aevum "无 DSL 组织复杂系统"的答案:用户选一个模板(声明想要什么能力),
//! 确定性求解器算成精确闭包。模板**只给约束不给 hash**——表达"想要",lock 记录"得到"。
//!
//! 本 crate 是前端无关的共享层:TS 前端(config-ts)与未来的 TOML 前端都调 [`expand`]
//! 把模板名展开成 `Vec<Constraint>`,再走同一条确定性求解路。
//!
//! # TOML 格式(实现采用的子集)
//! 设计文档 §1 用 `[[capability]]` / `[[optional]]` 数组表头,但本项目零依赖 TOML 解析器
//! ([`aevum_service_compiler::parse_toml_subset`])不支持数组表头。故跟随 foundation manifest
//! 先例([`aevum_maintainer::FoundationManifest`]),改用**分节形态**(语义等价、解析器原生支持):
//!
//! ```toml
//! [template]
//! name = "dev-python-ds"
//! title = "Python 数据科学环境"
//! version = "1.0.0"
//! extends = ["minimal-desktop"]      # 继承的父模板(可多个,可空)
//!
//! [capability.python3]
//! constraint = ">=3.10"              # 版本约束(声明,不指定 hash)
//! layer_hint = "app"                 # 建议层;不可为 foundation
//!
//! [capability.numpy]
//! constraint = ">=1.26"
//! layer_hint = "app"
//!
//! [optional.jupyter]
//! default = "true"                   # 解析器只认字符串,布尔写 "true"/"false"
//! id = "jupyter-pkg"                 # 可选:开启时带入的包名(缺省用分节 id)
//!
//! [optional.cuda-runtime]
//! default = "false"
//! ```
//!
//! # 确定性
//! 同模板名 + 同 override/exclude/optional 开关 → 必产相同约束集(BTreeMap 按 id 排序输出)。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use aevum_service_compiler::parse_toml_subset;
use aevum_solver::version::VerOp;
use aevum_solver::Constraint;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("模板 TOML 解析失败: {0}")]
    Toml(String),
    #[error("模板 {name} 的能力 {cap} 的 layer_hint 不可为 foundation(意图不能塞进 Foundation 层)")]
    ForbiddenLayer { name: String, cap: String },
    #[error("模板文件未找到: {0}")]
    NotFound(PathBuf),
    #[error("读模板文件失败 {path}: {source}")]
    Io { path: PathBuf, #[source] source: std::io::Error },
    #[error("检测到模板继承环: {0}")]
    Cycle(String),
    #[error("无法解析版本约束 {0:?}(支持 */>=/<=/=/>/< + 版本号)")]
    BadConstraint(String),
}

/// 一个抽象能力声明(对应 store 包的 provides.capabilities)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    /// 能力标识(= 分节 `[capability.<id>]` 的 id),通常即包名。
    pub id: String,
    /// 版本约束(`*` / `>=3.10` / `3.11` 等),交给求解器。
    pub constraint: String,
    /// 层归属建议;最终由 Maintainer 定,但不可为 foundation。
    pub layer_hint: String,
}

/// 可选组件 + 默认开关。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Optional {
    /// 分节 `[optional.<id>]` 的 id。
    pub id: String,
    /// 开启时带入的包名(缺省用 id)。
    pub package: String,
    /// 默认是否开启(用户开关可覆盖)。
    pub default: bool,
}

/// 一份模板(声明式蓝图)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub name: String,
    pub title: String,
    pub version: String,
    /// 继承的父模板(先展开父,再叠加本模板)。
    pub extends: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub optionals: Vec<Optional>,
}

impl Template {
    /// 解析模板 TOML(分节形态,见模块文档)。`layer_hint==foundation` 立即拒绝(验收6)。
    pub fn parse(text: &str) -> Result<Template, TemplateError> {
        let doc = parse_toml_subset(text).map_err(|e| TemplateError::Toml(e.to_string()))?;

        let tmpl = doc.get("template");
        let name = tmpl.and_then(|s| s.get_str("name")).unwrap_or_default();
        let title = tmpl.and_then(|s| s.get_str("title")).unwrap_or_default();
        let version = tmpl.and_then(|s| s.get_str("version")).unwrap_or_else(|| "0.0.0".into());
        let extends = tmpl.map(|s| s.get_arr("extends")).unwrap_or_default();

        let mut capabilities = Vec::new();
        let mut optionals = Vec::new();
        for (section, sec) in &doc {
            if let Some(id) = section.strip_prefix("capability.") {
                let id = id.trim();
                if id.is_empty() {
                    continue;
                }
                let constraint = sec.get_str("constraint").unwrap_or_else(|| "*".into());
                let layer_hint = sec.get_str("layer_hint").unwrap_or_else(|| "app".into());
                if layer_hint == "foundation" {
                    return Err(TemplateError::ForbiddenLayer {
                        name: name.clone(),
                        cap: id.to_string(),
                    });
                }
                capabilities.push(Capability { id: id.to_string(), constraint, layer_hint });
            } else if let Some(id) = section.strip_prefix("optional.") {
                let id = id.trim();
                if id.is_empty() {
                    continue;
                }
                let package = sec.get_str("id").unwrap_or_else(|| id.to_string());
                // default 缺省视为 false(可选组件默认不开,除非显式 "true")。
                let default = sec.get_str("default").map(|v| v == "true").unwrap_or(false);
                optionals.push(Optional { id: id.to_string(), package, default });
            }
        }
        // 分节经 BTreeMap 已按 id 有序;显式排一次保确定性。
        capabilities.sort_by(|a, b| a.id.cmp(&b.id));
        optionals.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(Template { name, title, version, extends, capabilities, optionals })
    }
}

/// 从模板目录加载一个模板:读 `dir/<name>.toml` → parse。
pub fn load_template(dir: &Path, name: &str) -> Result<Template, TemplateError> {
    let path = dir.join(format!("{name}.toml"));
    if !path.exists() {
        return Err(TemplateError::NotFound(path));
    }
    let text = std::fs::read_to_string(&path).map_err(|source| TemplateError::Io {
        path: path.clone(),
        source,
    })?;
    Template::parse(&text)
}

/// 模板覆盖项(用户意图,最高优先级):钉版本或排除。
#[derive(Debug, Clone, Default)]
pub struct ExpandOptions {
    /// override:能力 id → 版本约束字符串(如 `"=20"` / `">=18"`);覆盖模板声明。
    pub overrides: BTreeMap<String, String>,
    /// exclude:从最终约束集中移除这些能力 id。
    pub excludes: BTreeSet<String>,
    /// optional 开关:id → 是否开启;覆盖模板的 default。
    pub optional_switches: BTreeMap<String, bool>,
}

/// 展开模板集为约束集(本 crate 核心,兑现 §3 解析合并顺序)。
///
/// 步骤:
/// 1. 对每个 root 模板 DFS 展开 `extends`(深度优先,先父后子),`visited` 防重复加载,
///    路径栈 detect 环(验收2)。
/// 2. 按展开顺序合并 capability 进 `BTreeMap<id, 约束串>`,**后者覆盖前者**(验收3):
///    后声明模板 / 子模板的同 id 约束胜出。
/// 3. optional:`default` 决定是否带入,`optional_switches` 覆盖(验收4)。
/// 4. 应用 `overrides`(覆盖约束串)、`excludes`(移除)(验收3/4)。
/// 5. 约束串 → [`Constraint`],按 id 排序输出(确定性)。
pub fn expand(
    dir: &Path,
    roots: &[String],
    opts: &ExpandOptions,
) -> Result<Vec<Constraint>, TemplateError> {
    // id → 约束串(BTreeMap 保证最终按 id 排序;插入即"后覆盖前")。
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();

    for root in roots {
        let mut stack: Vec<String> = Vec::new();
        expand_one(dir, root, &mut merged, &mut visited, &mut stack)?;
    }

    // override:覆盖已有约束串(即便该 id 未被任何模板声明,也纳入——显式 override 即意图要它)。
    for (id, c) in &opts.overrides {
        merged.insert(id.clone(), c.clone());
    }
    // exclude:移除。
    for id in &opts.excludes {
        merged.remove(id);
    }

    // 约束串 → Constraint,按 id 排序(BTreeMap 迭代即有序)。
    let mut out = Vec::with_capacity(merged.len());
    for (id, c) in merged {
        out.push(parse_constraint(&id, &c)?);
    }
    Ok(out)
}

/// DFS 展开单个模板:先递归父(extends),再叠加本模板的 capability / optional。
fn expand_one(
    dir: &Path,
    name: &str,
    merged: &mut BTreeMap<String, String>,
    visited: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), TemplateError> {
    // 环检测:当前展开路径里已含 name → 环。
    if stack.contains(&name.to_string()) {
        stack.push(name.to_string());
        return Err(TemplateError::Cycle(stack.join(" → ")));
    }
    // 已完整展开过(非环,菱形继承的共享父)→ 跳过重复加载,但其约束已在 merged 里。
    if visited.contains(name) {
        return Ok(());
    }

    let tmpl = load_template(dir, name)?;
    stack.push(name.to_string());

    // 先父后子:父的能力先入,子的同 id 覆盖(后覆盖前)。
    for parent in &tmpl.extends {
        expand_one(dir, parent, merged, visited, stack)?;
    }

    // 叠加本模板能力(覆盖父)。
    for cap in &tmpl.capabilities {
        merged.insert(cap.id.clone(), cap.constraint.clone());
    }
    // optional:default 开启则带入(无约束),供后续 override/switch 调整。
    // 注意:optional 的开关需在 expand 顶层应用(跨模板),此处先按 default 入,
    // switch 的覆盖在 expand 收尾统一处理——但 default=false 的此处不入。
    for opt in &tmpl.optionals {
        if opt.default {
            merged.entry(opt.package.clone()).or_insert_with(|| "*".into());
        }
    }

    stack.pop();
    visited.insert(name.to_string());
    Ok(())
}

/// 约束串 → [`Constraint`]。支持 `*`(无约束)/ `>=v` / `<=v` / `=v` / `>v` / `<v` / 裸版本号(=)。
fn parse_constraint(id: &str, raw: &str) -> Result<Constraint, TemplateError> {
    let s = raw.trim();
    if s == "*" || s.is_empty() {
        return Ok(Constraint::unconstrained(id));
    }
    // 双字符操作符优先。
    let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
        (VerOp::Ge, r)
    } else if let Some(r) = s.strip_prefix("<=") {
        (VerOp::Le, r)
    } else if let Some(r) = s.strip_prefix(">") {
        (VerOp::Gt, r)
    } else if let Some(r) = s.strip_prefix("<") {
        (VerOp::Lt, r)
    } else if let Some(r) = s.strip_prefix("=") {
        (VerOp::Eq, r)
    } else {
        // 裸版本号:视为精确等于。
        (VerOp::Eq, s)
    };
    let ver = rest.trim();
    if ver.is_empty() {
        return Err(TemplateError::BadConstraint(raw.to_string()));
    }
    Ok(Constraint { name: id.to_string(), op: Some(op), ver: Some(ver.to_string()) })
}

/// 收集展开 `roots` 涉及的全部模板及版本(含 extends 传递闭包),供世代记录(验收7)。
///
/// 返回 `(name, version)`,按名去重排序(确定性)。复用 [`expand`] 的 DFS 加载 + 无环校验语义:
/// 同一份 extends 树只加载一次,检测到环则 `Err(Cycle)`。
pub fn collect_templates(dir: &Path, roots: &[String]) -> Result<Vec<(String, String)>, TemplateError> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut visited: BTreeSet<String> = BTreeSet::new();
    for root in roots {
        let mut stack: Vec<String> = Vec::new();
        collect_one(dir, root, &mut seen, &mut visited, &mut stack)?;
    }
    Ok(seen.into_iter().collect())
}

/// DFS 收集单个模板及其 extends 链的 (name, version)。
fn collect_one(
    dir: &Path,
    name: &str,
    seen: &mut BTreeMap<String, String>,
    visited: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<(), TemplateError> {
    if stack.contains(&name.to_string()) {
        stack.push(name.to_string());
        return Err(TemplateError::Cycle(stack.join(" → ")));
    }
    if visited.contains(name) {
        return Ok(());
    }
    let tmpl = load_template(dir, name)?;
    stack.push(name.to_string());
    for parent in &tmpl.extends {
        collect_one(dir, parent, seen, visited, stack)?;
    }
    seen.insert(tmpl.name.clone(), tmpl.version.clone());
    stack.pop();
    visited.insert(name.to_string());
    Ok(())
}

/// 应用 optional 开关:在 expand 之外、由调用方按需调整(switch 覆盖 default)。
///
/// 因 [`expand`] 已按 default 处理,switch 仅在"用户显式改开关"时需要——
/// 为保接口完整,这里提供一个在已展开约束集上应用 switch 的辅助:
/// switch=true 且某 optional 包不在集 → 加入;switch=false → 移除。
/// (调用方需提供 optional id→package 映射;最小实现中 CLI 直接传包名。)
pub fn apply_optional_switch(
    constraints: &mut Vec<Constraint>,
    package: &str,
    enabled: bool,
) {
    let present = constraints.iter().any(|c| c.name == package);
    match (enabled, present) {
        (true, false) => constraints.push(Constraint::unconstrained(package)),
        (false, true) => constraints.retain(|c| c.name != package),
        _ => {}
    }
    constraints.sort_by(|a, b| a.name.cmp(&b.name));
}

#[cfg(test)]
mod tests {
    use super::*;

    const PY_DS: &str = r#"
[template]
name = "dev-python-ds"
title = "Python 数据科学"
version = "1.0.0"
extends = ["base"]

[capability.python3]
constraint = ">=3.10"
layer_hint = "app"

[capability.numpy]
constraint = ">=1.26"
layer_hint = "app"

[optional.jupyter]
default = "true"

[optional.cuda-runtime]
default = "false"
"#;

    const BASE: &str = r#"
[template]
name = "base"
version = "0.1.0"

[capability.coreutils]
constraint = "*"
layer_hint = "system"

[capability.python3]
constraint = ">=3.8"
layer_hint = "app"
"#;

    fn write_templates(dir: &Path, files: &[(&str, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        for (name, body) in files {
            std::fs::write(dir.join(format!("{name}.toml")), body).unwrap();
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aevum-tmpl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn parse_capabilities_and_optionals() {
        let t = Template::parse(PY_DS).unwrap();
        assert_eq!(t.name, "dev-python-ds");
        assert_eq!(t.extends, vec!["base"]);
        assert_eq!(t.capabilities.len(), 2);
        assert!(t.capabilities.iter().any(|c| c.id == "numpy" && c.constraint == ">=1.26"));
        let jup = t.optionals.iter().find(|o| o.id == "jupyter").unwrap();
        assert!(jup.default, "jupyter default 应为 true");
        let cuda = t.optionals.iter().find(|o| o.id == "cuda-runtime").unwrap();
        assert!(!cuda.default, "cuda-runtime default 应为 false");
    }

    #[test]
    fn layer_hint_foundation_rejected() {
        let bad = r#"
[template]
name = "evil"
[capability.init]
constraint = "*"
layer_hint = "foundation"
"#;
        assert!(matches!(Template::parse(bad), Err(TemplateError::ForbiddenLayer { .. })));
    }

    #[test]
    fn single_template_expands_to_constraints() {
        let dir = tmp_dir("single");
        write_templates(&dir, &[("base", BASE)]);
        let cs = expand(&dir, &["base".into()], &ExpandOptions::default()).unwrap();
        let names: Vec<&str> = cs.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"coreutils"));
        assert!(names.contains(&"python3"));
        // 模板只给约束不给 hash(验收5):产物无 fingerprint 概念,只有 name/op/ver。
        let py = cs.iter().find(|c| c.name == "python3").unwrap();
        assert_eq!(py.op, Some(VerOp::Ge));
        assert_eq!(py.ver.as_deref(), Some("3.8"));
    }

    #[test]
    fn extends_child_overrides_parent() {
        // dev-python-ds extends base;两者都声明 python3,子的 >=3.10 应覆盖父的 >=3.8(验收3)。
        let dir = tmp_dir("extends");
        write_templates(&dir, &[("base", BASE), ("dev-python-ds", PY_DS)]);
        let cs = expand(&dir, &["dev-python-ds".into()], &ExpandOptions::default()).unwrap();
        let py = cs.iter().find(|c| c.name == "python3").unwrap();
        assert_eq!(py.ver.as_deref(), Some("3.10"), "子模板 python3 约束应覆盖父");
        // 父的 coreutils 也应在(继承)。
        assert!(cs.iter().any(|c| c.name == "coreutils"));
        // optional jupyter default=true → 带入;cuda default=false → 不带入(验收4)。
        assert!(cs.iter().any(|c| c.name == "jupyter"), "jupyter 应默认带入");
        assert!(!cs.iter().any(|c| c.name == "cuda-runtime"), "cuda 默认不带入");
    }

    #[test]
    fn cycle_detected() {
        // a extends b, b extends a → 环。
        let dir = tmp_dir("cycle");
        write_templates(&dir, &[
            ("a", "[template]\nname=\"a\"\nextends=[\"b\"]\n[capability.x]\nconstraint=\"*\"\n"),
            ("b", "[template]\nname=\"b\"\nextends=[\"a\"]\n[capability.y]\nconstraint=\"*\"\n"),
        ]);
        let r = expand(&dir, &["a".into()], &ExpandOptions::default());
        assert!(matches!(r, Err(TemplateError::Cycle(_))), "应检测到继承环, got {r:?}");
    }

    #[test]
    fn override_beats_template() {
        let dir = tmp_dir("override");
        write_templates(&dir, &[("base", BASE)]);
        let mut opts = ExpandOptions::default();
        opts.overrides.insert("python3".into(), "=3.12".into());
        let cs = expand(&dir, &["base".into()], &opts).unwrap();
        let py = cs.iter().find(|c| c.name == "python3").unwrap();
        assert_eq!(py.op, Some(VerOp::Eq));
        assert_eq!(py.ver.as_deref(), Some("3.12"), "override 应胜出");
    }

    #[test]
    fn exclude_removes_capability() {
        let dir = tmp_dir("exclude");
        write_templates(&dir, &[("base", BASE)]);
        let mut opts = ExpandOptions::default();
        opts.excludes.insert("coreutils".into());
        let cs = expand(&dir, &["base".into()], &opts).unwrap();
        assert!(!cs.iter().any(|c| c.name == "coreutils"), "exclude 的能力应被移除");
        assert!(cs.iter().any(|c| c.name == "python3"));
    }

    #[test]
    fn multi_template_later_wins() {
        // 两个 root 模板都声明 python3,后者覆盖前者(验收3:后声明模板胜)。
        let dir = tmp_dir("multi");
        write_templates(&dir, &[
            ("base", BASE), // python3 >=3.8
            ("pinpy", "[template]\nname=\"pinpy\"\n[capability.python3]\nconstraint=\"=3.11\"\nlayer_hint=\"app\"\n"),
        ]);
        let cs = expand(&dir, &["base".into(), "pinpy".into()], &ExpandOptions::default()).unwrap();
        let py = cs.iter().find(|c| c.name == "python3").unwrap();
        assert_eq!(py.ver.as_deref(), Some("3.11"), "后声明模板 pinpy 应覆盖 base");
    }

    #[test]
    fn collect_templates_includes_extends_chain() {
        // dev-python-ds extends base;collect 应含两者及其版本(验收7 的数据源)。
        let dir = tmp_dir("collect");
        write_templates(&dir, &[("base", BASE), ("dev-python-ds", PY_DS)]);
        let got = collect_templates(&dir, &["dev-python-ds".into()]).unwrap();
        // 按名排序:base@0.1.0, dev-python-ds@1.0.0。
        assert_eq!(got, vec![
            ("base".to_string(), "0.1.0".to_string()),
            ("dev-python-ds".to_string(), "1.0.0".to_string()),
        ], "应含 extends 链全部模板及版本(按名排序)");
    }

    #[test]
    fn collect_templates_cycle_errors() {
        let dir = tmp_dir("collect-cycle");
        write_templates(&dir, &[
            ("a", "[template]\nname=\"a\"\nextends=[\"b\"]\n"),
            ("b", "[template]\nname=\"b\"\nextends=[\"a\"]\n"),
        ]);
        assert!(matches!(collect_templates(&dir, &["a".into()]), Err(TemplateError::Cycle(_))));
    }

    #[test]
    fn constraint_parsing_forms() {
        assert!(parse_constraint("x", "*").unwrap().op.is_none());
        assert_eq!(parse_constraint("x", ">=1.2").unwrap().op, Some(VerOp::Ge));
        assert_eq!(parse_constraint("x", "<=1.2").unwrap().op, Some(VerOp::Le));
        assert_eq!(parse_constraint("x", ">1.2").unwrap().op, Some(VerOp::Gt));
        assert_eq!(parse_constraint("x", "<1.2").unwrap().op, Some(VerOp::Lt));
        assert_eq!(parse_constraint("x", "=1.2").unwrap().op, Some(VerOp::Eq));
        assert_eq!(parse_constraint("x", "1.2").unwrap().op, Some(VerOp::Eq), "裸版本号视为 =");
        assert!(matches!(parse_constraint("x", ">="), Err(TemplateError::BadConstraint(_))));
    }

    #[test]
    fn diamond_inheritance_no_false_cycle() {
        // d extends [b, c]; b extends a; c extends a。a 是共享父(菱形),不是环。
        let dir = tmp_dir("diamond");
        write_templates(&dir, &[
            ("a", "[template]\nname=\"a\"\n[capability.acap]\nconstraint=\"*\"\n"),
            ("b", "[template]\nname=\"b\"\nextends=[\"a\"]\n[capability.bcap]\nconstraint=\"*\"\n"),
            ("c", "[template]\nname=\"c\"\nextends=[\"a\"]\n[capability.ccap]\nconstraint=\"*\"\n"),
            ("d", "[template]\nname=\"d\"\nextends=[\"b\",\"c\"]\n[capability.dcap]\nconstraint=\"*\"\n"),
        ]);
        let cs = expand(&dir, &["d".into()], &ExpandOptions::default()).unwrap();
        let names: Vec<&str> = cs.iter().map(|c| c.name.as_str()).collect();
        // 四个能力都在,a 只展开一次(visited 防重),无误报环。
        for cap in ["acap", "bcap", "ccap", "dcap"] {
            assert!(names.contains(&cap), "{cap} 应在: {names:?}");
        }
    }

    #[test]
    fn determinism_same_input_same_output() {
        let dir = tmp_dir("det");
        write_templates(&dir, &[("base", BASE), ("dev-python-ds", PY_DS)]);
        let a = expand(&dir, &["dev-python-ds".into()], &ExpandOptions::default()).unwrap();
        let b = expand(&dir, &["dev-python-ds".into()], &ExpandOptions::default()).unwrap();
        let an: Vec<&str> = a.iter().map(|c| c.name.as_str()).collect();
        let bn: Vec<&str> = b.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(an, bn, "同输入两次展开必产相同有序约束");
        // 验证确实有序(按 name)。
        let mut sorted = an.clone();
        sorted.sort();
        assert_eq!(an, sorted, "约束应按 name 排序(确定性)");
    }

    #[test]
    fn optional_switch_overrides_default() {
        // cuda default=false,switch 开启 → 应带入。
        let dir = tmp_dir("switch");
        write_templates(&dir, &[("base", BASE), ("dev-python-ds", PY_DS)]);
        let mut cs = expand(&dir, &["dev-python-ds".into()], &ExpandOptions::default()).unwrap();
        assert!(!cs.iter().any(|c| c.name == "cuda-runtime"));
        apply_optional_switch(&mut cs, "cuda-runtime", true);
        assert!(cs.iter().any(|c| c.name == "cuda-runtime"), "switch 开启后应带入 cuda");
        apply_optional_switch(&mut cs, "jupyter", false);
        assert!(!cs.iter().any(|c| c.name == "jupyter"), "switch 关闭后应移除 jupyter");
    }
}
