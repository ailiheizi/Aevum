//! 确定性闭包求解器(无 AI、无随机、无时钟)。
//!
//! 直译自 PoC-3 `solver.py`:`load_index` / `parse_atom` / `pick_version` /
//! `resolve` / `build_lock`。这是 ADR-0003 边界1 的兑现——
//! **AI 产约束/意图,确定性求解器算具体 hash;可复现只来自 lock,不来自 AI。**
//!
//! # 确定性保证(关键,改这里要保住这四条)
//! 1. 候选版本:满足约束者中选「版本号最大」(确定偏序,见 [`version`])。
//! 2. 闭包展开:按包名排序的工作队列。
//! 3. alternatives `a|b`:取第一个在索引/虚包中存在的。
//! 4. `closure_id` = 对最终 `(name,version,fingerprint)` 排序后取摘要。
//!
//! 同输入 + 同索引快照 → 必然同输出。PoC-3 用 442 真实 Debian 包验证零未解析、三次一致。

pub mod version;

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use version::{ver_satisfies, deb_ver_cmp, VerOp};

/// 索引中的一条包记录(对应 PoC 的 `rec`)。
#[derive(Debug, Clone)]
pub struct PackageRecord {
    pub version: String,
    /// `Depends` 原始字段(逗号分隔的原子,原子内可含 `a|b` alternatives)。
    pub depends: String,
    /// `Pre-Depends` 原始字段。
    pub predepends: String,
    pub filename: String,
    pub sha256: String,
}

/// 包索引:`name -> [记录]`,加上 `虚包名 -> [真实包名]` 的 provides 映射。
#[derive(Debug, Default)]
pub struct Index {
    pub by_name: HashMap<String, Vec<PackageRecord>>,
    pub provides: HashMap<String, Vec<String>>,
}

impl Index {
    /// 从 Debian `Packages` 文本构建索引(直译自 PoC-3 `load_index`)。
    ///
    /// 格式:段落式,空行分隔一个包;`Key: Value`;续行以空格/制表符开头。
    /// 提取 Package/Version/Depends/Pre-Depends/Filename/SHA256/Provides。
    /// Provides 逗号分隔、去 `(...)` 版本注解 → 虚包映射(`虚包名 -> [真实包名]`)。
    ///
    /// 续行(长 Depends 跨行)按 PoC-3 简化跳过——真实 Debian 索引中依赖字段极少跨行。
    /// 输入应是已解压的纯文本(gzip 由外部 `gunzip` 处理,不引 flate2 依赖)。
    pub fn from_packages_str(text: &str) -> Index {
        let mut idx = Index::default();
        let mut cur: HashMap<String, String> = HashMap::new();

        // flush 当前段落为一条记录。
        fn flush(idx: &mut Index, cur: &HashMap<String, String>) {
            let name = match cur.get("Package") {
                Some(n) if !n.is_empty() => n.clone(),
                _ => return,
            };
            let rec = PackageRecord {
                version: cur.get("Version").cloned().unwrap_or_else(|| "0".into()),
                depends: cur.get("Depends").cloned().unwrap_or_default(),
                predepends: cur.get("Pre-Depends").cloned().unwrap_or_default(),
                filename: cur.get("Filename").cloned().unwrap_or_default(),
                sha256: cur.get("SHA256").cloned().unwrap_or_default(),
            };
            idx.by_name.entry(name.clone()).or_default().push(rec);
            // Provides:逗号分隔,去 `(...)` 版本,建虚包映射。
            if let Some(provides) = cur.get("Provides") {
                for prov in provides.split(',') {
                    let p = prov.split('(').next().unwrap_or("").trim();
                    if !p.is_empty() {
                        idx.provides.entry(p.to_string()).or_default().push(name.clone());
                    }
                }
            }
        }

        for line in text.lines() {
            if line.trim().is_empty() {
                flush(&mut idx, &cur);
                cur.clear();
            } else if line.starts_with(' ') || line.starts_with('\t') {
                // 续行:简化跳过(PoC-3 同款,真实索引依赖字段极少跨行)。
                continue;
            } else if let Some((k, v)) = line.split_once(':') {
                cur.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        flush(&mut idx, &cur); // 末段无尾随空行时补 flush
        idx
    }

    /// 索引内不同包名数(诊断/测试用)。
    pub fn package_count(&self) -> usize {
        self.by_name.len()
    }
}

/// 一个依赖约束原子:包名 + 可选 (运算符, 版本)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    pub name: String,
    pub op: Option<VerOp>,
    pub ver: Option<String>,
}

impl Constraint {
    pub fn unconstrained(name: impl Into<String>) -> Self {
        Constraint { name: name.into(), op: None, ver: None }
    }
}

/// override:覆盖某包版本约束,或将其排除。
#[derive(Debug, Clone)]
pub enum Override {
    Pin { op: VerOp, ver: String },
    Exclude,
}

/// 解析单个依赖原子的所有 alternatives。对应 PoC 的 `parse_alternatives`。
///
/// `"libssl3 (>= 3.0) | libssl1.1"` → `[Constraint{libssl3,>=,3.0}, Constraint{libssl1.1,..}]`。
pub fn parse_alternatives(atom: &str) -> Vec<Constraint> {
    atom.split('|')
        .filter_map(|alt| parse_one(alt.trim()))
        .collect()
}

/// 解析单个候选 `name (op ver)`。无法解析返回 `None`。
fn parse_one(s: &str) -> Option<Constraint> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // 拆出括号约束部分
    if let Some(paren_start) = s.find('(') {
        let name = s[..paren_start].trim();
        if name.is_empty() {
            return None;
        }
        let inner = s[paren_start + 1..]
            .trim_end_matches(')')
            .trim_end()
            .trim_end_matches(')')
            .trim();
        // inner 形如 ">= 3.0";切出运算符与版本
        let (op_str, ver) = split_op_ver(inner);
        return Some(Constraint {
            name: valid_name(name)?,
            op: VerOp::parse(op_str),
            ver: if ver.is_empty() { None } else { Some(ver.to_string()) },
        });
    }
    // 无约束
    Some(Constraint {
        name: valid_name(s)?,
        op: None,
        ver: None,
    })
}

/// 校验包名只含合法字符(对应 PoC 的 DEP_RE `[a-z0-9+.\-]+`),并去掉冒号架构后缀(如 `:any`)。
fn valid_name(name: &str) -> Option<String> {
    let name = name.split(':').next().unwrap_or(name).trim();
    if name.is_empty() {
        return None;
    }
    if name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'))
    {
        Some(name.to_string())
    } else {
        None
    }
}

/// 把 `">= 3.0"` 切成 `(">=", "3.0")`。运算符是前缀里的 `<>=` 字符。
fn split_op_ver(inner: &str) -> (&str, &str) {
    let inner = inner.trim();
    let op_end = inner
        .find(|c: char| !matches!(c, '<' | '>' | '='))
        .unwrap_or(inner.len());
    let op = &inner[..op_end];
    let ver = inner[op_end..].trim();
    (op, ver)
}

/// 确定性版本选择:满足约束的版本中选最大者。对应 PoC 的 `pick_version`。
pub fn pick_version<'a>(
    index: &'a Index,
    name: &str,
    op: Option<VerOp>,
    ver: Option<&str>,
) -> Option<&'a PackageRecord> {
    let cands = index.by_name.get(name)?;
    let mut ok: Vec<&PackageRecord> = cands
        .iter()
        .filter(|rec| match (op, ver) {
            (Some(o), Some(v)) => ver_satisfies(&rec.version, o, v),
            _ => true,
        })
        .collect();
    if ok.is_empty() {
        return None;
    }
    // 选最大版本(确定性偏序)
    ok.sort_by(|a, b| deb_ver_cmp(&a.version, &b.version));
    ok.into_iter().last()
}

/// repair 方案 A:在 index 候选里找一个**同时满足全部约束**的最高版本(ai/02 §2 方案A)。
///
/// `constraints` 是某包被收集到的全部带版本约束 `(op, ver)`。确定性纯函数:
/// 遍历该包所有候选版本,保留满足**每一条**约束者,取其中最高版本。
/// 返回 None 表示无单一共存版本(方案 A 不适用,需转 B/C)。
pub fn find_satisfying_version(
    index: &Index,
    name: &str,
    constraints: &[(VerOp, String)],
) -> Option<String> {
    let cands = index.by_name.get(name)?;
    let mut ok: Vec<&PackageRecord> = cands
        .iter()
        .filter(|rec| {
            constraints
                .iter()
                .all(|(op, ver)| ver_satisfies(&rec.version, *op, ver))
        })
        .collect();
    if ok.is_empty() {
        return None;
    }
    ok.sort_by(|a, b| deb_ver_cmp(&a.version, &b.version));
    ok.into_iter().last().map(|r| r.version.clone())
}

/// 从一条 `Depends`/`Pre-Depends` 字段里,提取针对 `dep_name` 的版本约束 `(op, ver)`。
/// 无该依赖、或该依赖无版本约束(裸 `dep`)→ None。alternatives(`a|b`)里命中 dep_name 的取其约束。
fn extract_constraint_for(depends: &str, dep_name: &str) -> Option<(VerOp, String)> {
    for atom in depends.split(',') {
        for alt in parse_alternatives(atom.trim()) {
            if alt.name == dep_name {
                if let (Some(op), Some(ver)) = (alt.op, alt.ver) {
                    return Some((op, ver));
                }
            }
        }
    }
    None
}

/// repair 方案 B(升级父包求兼容):方案A 对 `dep_name` 无解时调用。
///
/// `parent_constraints` 是「(父包名, 该父包当前对 dep 的 (op,ver))」列表;
/// `other_constraints` 是 dep 上不来自这些父包的其它约束(顶层/其它父包,固定不动)。
///
/// 逐个父包尝试:枚举该父包的更高候选版本,提取其 Depends 对 dep 的新约束,
/// 用「新约束 + 其它父包当前约束 + other」跑 [`find_satisfying_version`];
/// 若得到共存版本 → 产一条建议(升级该父包到该版本)。确定性:父包按版本升序、取第一个可解的。
pub fn suggest_upgrade_parent(
    index: &Index,
    dep_name: &str,
    parent_constraints: &[(String, (VerOp, String))],
    other_constraints: &[(VerOp, String)],
) -> Option<RepairSuggestionB> {
    for (i, (parent, _cur)) in parent_constraints.iter().enumerate() {
        let cands = match index.by_name.get(parent) {
            Some(c) => c,
            None => continue,
        };
        // 父包候选按版本升序,确定性地找最低的"能解"的升级版本。
        let mut versions: Vec<&PackageRecord> = cands.iter().collect();
        versions.sort_by(|a, b| deb_ver_cmp(&a.version, &b.version));
        for prec in versions {
            // 该父包版本对 dep 的新约束;若该版本不再约束 dep(裸依赖),视为无约束(不加入)。
            let new_dep_constraint = extract_constraint_for(&prec.depends, dep_name)
                .or_else(|| extract_constraint_for(&prec.predepends, dep_name));

            // 组装 dep 的约束全集:其它父包的当前约束 + other + 本父包的新约束。
            let mut combined: Vec<(VerOp, String)> = Vec::new();
            for (j, (_p, c)) in parent_constraints.iter().enumerate() {
                if j != i {
                    combined.push(c.clone());
                }
            }
            combined.extend(other_constraints.iter().cloned());
            if let Some(nc) = &new_dep_constraint {
                combined.push(nc.clone());
            }

            if let Some(v) = find_satisfying_version(index, dep_name, &combined) {
                return Some(RepairSuggestionB {
                    dependency: dep_name.to_string(),
                    parent: parent.clone(),
                    upgrade_parent_to: prec.version.clone(),
                    dependency_version: v,
                });
            }
        }
    }
    None
}

/// repair 方案 C(保留两份):A/B 无解时,看冲突依赖能否拆成两组、各组各有满足版本(版本不同)。
///
/// `constraint_sources` 是该依赖的全部 `(op, ver, source)`。做法(确定性):
/// 1. 按"约束满足的最高版本"给每条约束分组——满足同一版本的来源归一组。
/// 2. 若出现 ≥2 个不同版本各能满足一部分来源 → 可保留两份(取版本最高的两组)。
/// 返回 None 表示连两份也分不出(任何版本都满足不了某来源,或所有来源指向同一版本)。
pub fn suggest_keep_two(
    index: &Index,
    dep_name: &str,
    constraint_sources: &[(VerOp, String, String)],
) -> Option<KeepTwoSuggestion> {
    // 每条约束单独求其满足的最高版本;按版本聚合来源。版本→来源集(BTreeMap 保确定性有序)。
    let mut by_version: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (op, ver, src) in constraint_sources {
        match find_satisfying_version(index, dep_name, &[(*op, ver.clone())]) {
            Some(v) => by_version.entry(v).or_default().push(src.clone()),
            None => return None, // 某约束任何版本都满足不了 → 连这一份都装不下,转方案D。
        }
    }
    // 去重各组来源。
    for srcs in by_version.values_mut() {
        srcs.sort();
        srcs.dedup();
    }
    if by_version.len() < 2 {
        return None; // 所有来源都能用同一版本 → 不是"保留两份"场景(A 本应已解)。
    }
    // 取版本最高的两组作为两份(确定性:版本降序取前二)。
    let mut versions: Vec<String> = by_version.keys().cloned().collect();
    versions.sort_by(|a, b| deb_ver_cmp(a, b));
    let vb = versions.pop().unwrap(); // 最高
    let va = versions.pop().unwrap(); // 次高
    Some(KeepTwoSuggestion {
        package: dep_name.to_string(),
        version_a: va.clone(),
        sources_a: by_version.get(&va).cloned().unwrap_or_default(),
        version_b: vb.clone(),
        sources_b: by_version.get(&vb).cloned().unwrap_or_default(),
    })
}

/// 求解诊断信息(对应 PoC 的 `diag`)。
#[derive(Debug, Default, Serialize)]
pub struct Diagnostics {
    pub unresolved: Vec<UnresolvedDep>,
    pub excluded_hit: Vec<String>,
    pub virtual_resolved: Vec<VirtualResolved>,
    pub alt_chosen: Vec<AltChosen>,
    /// 版本冲突:同一包被互斥约束要求(已选版本无法满足后来的约束)。
    /// repair 流程(ai/02)的触发依据;此前 solver 静默取其一,现显式留痕。
    pub conflicts: Vec<VersionConflict>,
    /// repair 方案 A(放宽约束求单一共存版本)的建议:对每个有冲突的包,
    /// 在 index 候选里找是否存在同时满足其全部约束的版本(ai/02 §2 方案A)。
    /// 仅产建议,不改 closure/lock(由上层决定是否采纳)。
    pub repair_suggestions: Vec<RepairSuggestion>,
    /// repair 方案 B(升级父包求兼容)的建议:方案A 无解(无单一共存版本)时,
    /// 尝试把"提出过严约束的父包"升级到一个对该依赖约束更宽松的版本,使冲突依赖有共存版本。
    /// 仅产建议、仅确定性可算的「升级父包」子情形(降级涉功能损失评估,属 AI 域,不在此)。
    pub repair_suggestions_b: Vec<RepairSuggestionB>,
    /// repair 方案 D(隔离失败,需用户取舍):方案A 无单一共存版本、且方案B 无可升级父包
    /// → 自动 repair 的确定性手段已穷尽(C 保留两份需运行时视图隔离,待定稿)。
    /// 如实告知用户"这些约束无法共存,需二选一",**绝不静默删除某一方**(ai/02 §2 方案D)。
    pub unrepairable: Vec<UnrepairableConflict>,
    /// repair 方案 C(保留两份)的建议:A/B 无单一/升级解,但冲突依赖能拆成两组、
    /// 各组在 index 各有满足版本(版本不同)→ 两份并存,各闭包引各自 hash(ai/02 §3)。
    /// 标 `needs_user_confirm`(占盘+各自安全更新,§3.3);本轮仅产建议,运行时视图隔离落地待后续。
    pub keep_two_suggestions: Vec<KeepTwoSuggestion>,
}

/// repair 方案 C 的建议:某冲突依赖保留两份,各组来源用各自版本。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct KeepTwoSuggestion {
    /// 冲突的依赖包名。
    pub package: String,
    /// 第一份版本 + 它满足的来源(消费这份的一方)。
    pub version_a: String,
    pub sources_a: Vec<String>,
    /// 第二份版本 + 它满足的来源(消费这份的另一方)。
    pub version_b: String,
    pub sources_b: Vec<String>,
}

/// repair 方案 D:某冲突依赖经 A/B 确定性手段都无法解决,需用户取舍。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UnrepairableConflict {
    /// 无法共存的依赖包名。
    pub package: String,
    /// 该包被收集到的全部互斥约束(文本形态),展示"哪些约束打架"。
    pub constraints: Vec<String>,
    /// 提出这些约束的来源(父包名 / `<template>`),供用户判断"二选一"取舍哪一方。
    pub sources: Vec<String>,
}

/// repair 方案 A 的建议:某冲突包是否存在"同时满足全部约束"的单一共存版本。
///
/// - `satisfying_version = Some(v)`:存在版本 v 同时满足该包的所有约束 → 放宽即可共存(最干净)。
/// - `satisfying_version = None`:index 中无单一版本同时满足 → 方案 A 不适用,需转方案 B/C(升降级/保留两份)。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RepairSuggestion {
    pub package: String,
    /// 该包被收集到的全部约束(`(op, ver)` 文本形态),用于诊断展示"为何冲突"。
    pub constraints: Vec<String>,
    /// 同时满足全部约束的最高版本;None 表示无单一共存版本。
    pub satisfying_version: Option<String>,
}

/// repair 方案 B 的建议:把某父包升级到新版本,使冲突依赖重获单一共存版本。
///
/// 例:`app-x@1.0` 依赖 `openssl(<<3.1)`、`app-y@2.0` 依赖 `openssl(>=3.2)` → openssl 无共存版本(A 失败);
/// 但 `app-x@1.2` 的 Depends 放宽为 `openssl(>=3.0)` → 升级 app-x 到 1.2 后 openssl 可选 3.2 共存。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RepairSuggestionB {
    /// 冲突的依赖包名(如 openssl)。
    pub dependency: String,
    /// 被建议升级的父包(如 app-x)。
    pub parent: String,
    /// 父包升级到的版本(其对 `dependency` 的约束更宽松)。
    pub upgrade_parent_to: String,
    /// 升级父包后,`dependency` 重获的共存版本。
    pub dependency_version: String,
}

/// 一处版本冲突:某包已选定 `chosen_version`,但 `source` 要求的约束 `(op ver)` 它不满足。
///
/// 例:closure 已选 `openssl@3.0.13`(满足先到的 `<<3.1`),
/// 但后来 `app-x` 要求 `openssl (>= 3.2)` → 3.0.13 不满足 → 记一条冲突。
/// 这是"约束互斥、无单一版本同时满足"的信号(ai/02 §1 触发 repair)。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct VersionConflict {
    /// 冲突的包名。
    pub package: String,
    /// closure 中已选定的版本(满足先到约束)。
    pub chosen_version: String,
    /// 与已选版本冲突的约束运算符(如 ">=")。
    pub required_op: String,
    /// 与已选版本冲突的约束版本(如 "3.2")。
    pub required_ver: String,
    /// 提出该冲突约束的来源(父包名;顶层意图为 "&lt;template&gt;")。
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct UnresolvedDep {
    pub name: String,
    pub op: Option<String>,
    pub ver: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VirtualResolved {
    pub virtual_name: String,
    pub chosen: String,
}

#[derive(Debug, Serialize)]
pub struct AltChosen {
    pub atom: String,
    pub chosen: String,
}

/// 求解结果:闭包(name -> 选中的记录索引信息)+ 诊断。
pub struct Resolution {
    /// name -> (version, fingerprint, filename),已按确定性选择定下。
    pub closure: BTreeMap<String, ResolvedPackage>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    pub version: String,
    pub filename: String,
    pub fingerprint: String,
}

/// 闭包求解主入口。对应 PoC 的 `resolve` + `build_lock` 的闭包部分。
///
/// - `template`:顶层意图(一组约束)。
/// - `overrides`:覆盖/排除。
///
/// 工作队列按包名排序处理,保证展开顺序确定。
pub fn resolve(
    index: &Index,
    template: &[Constraint],
    overrides: &HashMap<String, Override>,
) -> Resolution {
    let excluded: HashSet<&String> = overrides
        .iter()
        .filter(|(_, v)| matches!(v, Override::Exclude))
        .map(|(k, _)| k)
        .collect();

    let mut closure: BTreeMap<String, ResolvedPackage> = BTreeMap::new();
    let mut diag = Diagnostics::default();
    let mut seen: HashSet<(String, Option<VerOp>, Option<String>)> = HashSet::new();
    // 收集每个包出现过的全部带版本约束(供 repair 方案A 算"单一共存版本")。
    // 按 name 有序、约束去重,保证建议确定性。
    let mut pkg_constraints: BTreeMap<String, Vec<(VerOp, String)>> = BTreeMap::new();
    // 同上但带「来源父包」(供 repair 方案B 溯源:升级哪个父包能解冲突)。
    let mut pkg_constraint_sources: BTreeMap<String, Vec<(VerOp, String, String)>> = BTreeMap::new();

    // 确定性工作队列:每轮按包名排序后取队首。
    // 元素携带「来源」(顶层意图为 "<template>",依赖展开为父包名),供冲突诊断溯源。
    let mut queue: Vec<(Constraint, String)> = template
        .iter()
        .map(|c| (c.clone(), "<template>".to_string()))
        .collect();

    while !queue.is_empty() {
        queue.sort_by(|a, b| a.0.name.cmp(&b.0.name));
        let (mut c, source) = queue.remove(0);

        if excluded.contains(&c.name) {
            diag.excluded_hit.push(c.name.clone());
            continue;
        }
        // 应用 override
        if let Some(Override::Pin { op, ver }) = overrides.get(&c.name) {
            c.op = Some(*op);
            c.ver = Some(ver.clone());
        }

        // 收集本包的带版本约束(供 repair 方案A;去重保确定性)。
        if let (Some(op), Some(ver)) = (c.op, c.ver.as_deref()) {
            let entry = pkg_constraints.entry(c.name.clone()).or_default();
            let pair = (op, ver.to_string());
            if !entry.contains(&pair) {
                entry.push(pair);
            }
            // 同时记带来源(供方案B 溯源父包);(op,ver,source) 去重。
            let se = pkg_constraint_sources.entry(c.name.clone()).or_default();
            let triple = (op, ver.to_string(), source.clone());
            if !se.contains(&triple) {
                se.push(triple);
            }
        }

        // 冲突检测:该包已在 closure,但本约束(带 op+ver)的已选版本不满足 →
        // 互斥约束,无单一版本同时满足(ai/02 §1 触发 repair)。留痕,不覆盖已选。
        if let (Some(existing), Some(op), Some(ver)) =
            (closure.get(&c.name), c.op, c.ver.as_deref())
        {
            if !ver_satisfies(&existing.version, op, ver) {
                diag.conflicts.push(VersionConflict {
                    package: c.name.clone(),
                    chosen_version: existing.version.clone(),
                    required_op: op_str(op),
                    required_ver: ver.to_string(),
                    source: source.clone(),
                });
                // 已记冲突:不再用本约束重选/覆盖(保持已选版本确定性),继续处理队列。
                continue;
            }
        }

        let key = (c.name.clone(), c.op, c.ver.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);

        // 选版本;失败则尝试虚包
        let mut name = c.name.clone();
        let rec = match pick_version(index, &name, c.op, c.ver.as_deref()) {
            Some(r) => Some(r),
            None => {
                if let Some(reals) = index.provides.get(&name) {
                    // 确定性:取字典序第一个真实包
                    let mut sorted = reals.clone();
                    sorted.sort();
                    let real = sorted[0].clone();
                    diag.virtual_resolved.push(VirtualResolved {
                        virtual_name: name.clone(),
                        chosen: real.clone(),
                    });
                    let r = pick_version(index, &real, None, None);
                    name = real;
                    r
                } else {
                    None
                }
            }
        };

        let rec = match rec {
            Some(r) => r,
            None => {
                diag.unresolved.push(UnresolvedDep {
                    name: name.clone(),
                    op: c.op.map(op_str),
                    ver: c.ver.clone(),
                });
                continue;
            }
        };

        // 已在闭包且版本相同则跳过
        if let Some(existing) = closure.get(&name) {
            if existing.version == rec.version {
                continue;
            }
        }
        closure.insert(
            name.clone(),
            ResolvedPackage {
                version: rec.version.clone(),
                filename: rec.filename.clone(),
                fingerprint: content_fingerprint(rec),
            },
        );

        // 展开依赖(Depends + Pre-Depends)
        let all_deps = format!("{},{}", rec.depends, rec.predepends);
        for atom in all_deps.split(',') {
            let atom = atom.trim();
            if atom.is_empty() {
                continue;
            }
            let alts = parse_alternatives(atom);
            if alts.is_empty() {
                continue;
            }
            let chosen = if alts.len() > 1 {
                // 确定性 alternatives:选第一个在索引/虚包中存在的
                let pick = alts.iter().find(|cand| {
                    index.by_name.contains_key(&cand.name)
                        || index.provides.contains_key(&cand.name)
                });
                if let Some(p) = pick {
                    diag.alt_chosen.push(AltChosen {
                        atom: atom.to_string(),
                        chosen: p.name.clone(),
                    });
                }
                pick.cloned()
            } else {
                Some(alts[0].clone())
            };
            if let Some(ch) = chosen {
                if excluded.contains(&ch.name) {
                    // 被排除的传递依赖:不入队,但留痕(去重),否则用户的 exclude 无可见反馈。
                    if !diag.excluded_hit.contains(&ch.name) {
                        diag.excluded_hit.push(ch.name.clone());
                    }
                } else {
                    // 来源 = 当前正在展开依赖的包名(供冲突诊断溯源)。
                    queue.push((ch, name.clone()));
                }
            }
        }
    }

    // repair 方案A:对每个有冲突的包,看是否存在同时满足其全部约束的单一版本。
    // 冲突包按名有序去重,保证建议确定性。
    let mut conflict_pkgs: Vec<String> =
        diag.conflicts.iter().map(|c| c.package.clone()).collect();
    conflict_pkgs.sort();
    conflict_pkgs.dedup();
    for pkg in conflict_pkgs {
        let constraints = pkg_constraints.get(&pkg).cloned().unwrap_or_default();
        let satisfying_version = find_satisfying_version(index, &pkg, &constraints);
        let constraint_texts: Vec<String> = constraints
            .iter()
            .map(|(op, ver)| format!("{} {}", op_str(*op), ver))
            .collect();
        let a_solved = satisfying_version.is_some();
        diag.repair_suggestions.push(RepairSuggestion {
            package: pkg.clone(),
            constraints: constraint_texts,
            satisfying_version,
        });

        // 方案A 无解 → 尝试方案B(升级某父包,使该依赖重获共存版本)。
        if !a_solved {
            // 拆来源:有具名父包的约束(可尝试升级)与其它约束(顶层/无名,固定)。
            let sources = pkg_constraint_sources.get(&pkg).cloned().unwrap_or_default();
            let mut parent_constraints: Vec<(String, (VerOp, String))> = Vec::new();
            let mut other_constraints: Vec<(VerOp, String)> = Vec::new();
            for (op, ver, src) in sources {
                if src == "<template>" || !index.by_name.contains_key(&src) {
                    other_constraints.push((op, ver));
                } else {
                    parent_constraints.push((src, (op, ver)));
                }
            }
            let b = suggest_upgrade_parent(index, &pkg, &parent_constraints, &other_constraints);
            match b {
                Some(b) => diag.repair_suggestions_b.push(b),
                None => {
                    // 方案B 无解 → 试方案C(保留两份);C 也分不出才落方案D。
                    let sources = pkg_constraint_sources.get(&pkg).cloned().unwrap_or_default();
                    if let Some(c) = suggest_keep_two(index, &pkg, &sources) {
                        diag.keep_two_suggestions.push(c);
                    } else {
                        // 方案D:A/B/C 确定性手段都无解 → 如实告知,需用户取舍(绝不静默删一方)。
                        let mut srcs: Vec<String> =
                            sources.into_iter().map(|(_, _, s)| s).collect();
                        srcs.sort();
                        srcs.dedup();
                        let constraint_texts: Vec<String> = pkg_constraints
                            .get(&pkg).cloned().unwrap_or_default()
                            .iter().map(|(op, ver)| format!("{} {}", op_str(*op), ver)).collect();
                        diag.unrepairable.push(UnrepairableConflict {
                            package: pkg.clone(),
                            constraints: constraint_texts,
                            sources: srcs,
                        });
                    }
                }
            }
        }
    }

    Resolution {
        closure,
        diagnostics: diag,
    }
}

/// 一次被应用的 repair 方案A:把某冲突包钉到其单一共存版本。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AppliedRepair {
    pub package: String,
    pub pinned_version: String,
}

/// 带 repair 的求解结果(方案A 自动应用)。
pub struct RepairedResolution {
    /// 最终求解结果(已应用所有可行的方案A 放宽)。
    pub resolution: Resolution,
    /// 本次自动应用的放宽列表(把哪些包钉到哪个共存版本)。
    pub applied: Vec<AppliedRepair>,
    /// 仍无法用方案A 解决的冲突包(无单一共存版本,需方案 B/C)。
    pub unresolved_conflicts: Vec<String>,
}

/// 方案A **自动应用**:循环求解,对有单一共存版本的冲突包钉死(`Pin(Eq, v)`)后重求解,
/// 直到无冲突或剩余冲突都无交集(需 B/C)。ai/02 §2 方案A 的完整闭环(放宽 → 重求解可用 lock)。
///
/// # 收敛性
/// 每轮至少把一个「有交集」的冲突包钉死;钉死后该包约束统一为 `= v`,不再产生该包的冲突。
/// 故「有交集冲突包」集合严格单调减少 → 最多 `初始冲突包数` 轮收敛。外加保护上限兜底。
///
/// `base_overrides` 为调用方原有的 override(排除/钉版),repair 的 Pin 叠加其上;
/// 若某包已被调用方 Pin,则不再被 repair 覆盖(尊重显式意图)。
pub fn resolve_with_repair(
    index: &Index,
    template: &[Constraint],
    base_overrides: &HashMap<String, Override>,
) -> RepairedResolution {
    let mut overrides: HashMap<String, Override> = base_overrides.clone();
    let mut applied: Vec<AppliedRepair> = Vec::new();
    // 保护上限:正常 ≤ 冲突包数轮收敛,+8 兜底防御意外。
    let max_rounds = template.len() + 8;

    for _ in 0..max_rounds {
        let res = resolve(index, template, &overrides);
        if res.diagnostics.conflicts.is_empty() {
            return RepairedResolution { resolution: res, applied, unresolved_conflicts: Vec::new() };
        }

        // 找一个「有交集且尚未被 Pin」的冲突包,钉到其共存版本(确定性:按建议顺序取第一个)。
        let next = res.diagnostics.repair_suggestions.iter().find(|s| {
            s.satisfying_version.is_some()
                && !matches!(overrides.get(&s.package), Some(Override::Pin { .. }))
        });

        match next {
            Some(s) => {
                let v = s.satisfying_version.clone().unwrap();
                overrides.insert(
                    s.package.clone(),
                    Override::Pin { op: VerOp::Eq, ver: v.clone() },
                );
                applied.push(AppliedRepair { package: s.package.clone(), pinned_version: v });
                // 继续下一轮重求解。
            }
            None => {
                // 无可放宽的冲突包(剩余冲突都无交集)→ 方案A 到此为止,余下交 B/C。
                let unresolved: Vec<String> = {
                    let mut v: Vec<String> =
                        res.diagnostics.conflicts.iter().map(|c| c.package.clone()).collect();
                    v.sort();
                    v.dedup();
                    v
                };
                return RepairedResolution { resolution: res, applied, unresolved_conflicts: unresolved };
            }
        }
    }

    // 触达保护上限(理论不应到达):返回最后一次求解,余下冲突标记为未解决。
    let res = resolve(index, template, &overrides);
    let mut unresolved: Vec<String> =
        res.diagnostics.conflicts.iter().map(|c| c.package.clone()).collect();
    unresolved.sort();
    unresolved.dedup();
    RepairedResolution { resolution: res, applied, unresolved_conflicts: unresolved }
}

fn op_str(op: VerOp) -> String {
    match op {
        VerOp::Ge => ">=",
        VerOp::Le => "<=",
        VerOp::Eq => "=",
        VerOp::Gt => ">>",
        VerOp::Lt => "<<",
    }
    .to_string()
}

/// 内容指纹:优先用索引提供的 SHA256(真实内容寻址),否则用版本占位。
/// 对应 PoC 的 `content_fingerprint`。
fn content_fingerprint(rec: &PackageRecord) -> String {
    if !rec.sha256.is_empty() {
        format!("sha256:{}", rec.sha256)
    } else {
        let mut h = Sha256::new();
        h.update(rec.version.as_bytes());
        format!("placeholder:{}", hex::encode(h.finalize()))
    }
}

/// 锁定的单个包(写入 lock 文件)。
#[derive(Debug, Serialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub fingerprint: String,
    pub filename: String,
}

/// 完整 lock 文件结构。
#[derive(Debug, Serialize)]
pub struct Lock {
    /// `closure_id` = 对排序后 `(name,version,fingerprint)` 取 sha256 前 16 hex。
    pub closure_id: String,
    pub package_count: usize,
    pub locked: Vec<LockedPackage>,
    pub diagnostics: Diagnostics,
}

/// 从求解结果构建 lock。对应 PoC 的 `build_lock` 收尾部分。
///
/// `closure_id` 的算法与 PoC 完全一致:`"clo-" + sha256(blob)[..16]`,
/// 其中 `blob` 是按包名排序后每行 `name@version#fingerprint` 拼接。
pub fn build_lock(res: Resolution) -> Lock {
    // closure 是 BTreeMap,迭代天然按 name 排序 → 确定性。
    let locked: Vec<LockedPackage> = res
        .closure
        .iter()
        .map(|(name, pkg)| LockedPackage {
            name: name.clone(),
            version: pkg.version.clone(),
            fingerprint: pkg.fingerprint.clone(),
            filename: pkg.filename.clone(),
        })
        .collect();

    let blob = locked
        .iter()
        .map(|x| format!("{}@{}#{}", x.name, x.version, x.fingerprint))
        .collect::<Vec<_>>()
        .join("\n");
    let mut h = Sha256::new();
    h.update(blob.as_bytes());
    let digest = hex::encode(h.finalize());
    let closure_id = format!("clo-{}", &digest[..16]);

    Lock {
        closure_id,
        package_count: locked.len(),
        locked,
        diagnostics: res.diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_index() -> Index {
        let mut by_name: HashMap<String, Vec<PackageRecord>> = HashMap::new();
        by_name.insert(
            "ripgrep".into(),
            vec![PackageRecord {
                version: "13.0.0".into(),
                depends: "libc6 (>= 2.34), libgcc-s1 (>= 4.2)".into(),
                predepends: String::new(),
                filename: "pool/ripgrep_13.0.0.deb".into(),
                sha256: "aaaa".into(),
            }],
        );
        by_name.insert(
            "libc6".into(),
            vec![
                PackageRecord {
                    version: "2.34".into(),
                    depends: String::new(),
                    predepends: String::new(),
                    filename: "pool/libc6_2.34.deb".into(),
                    sha256: "bbbb".into(),
                },
                PackageRecord {
                    version: "2.36".into(),
                    depends: String::new(),
                    predepends: String::new(),
                    filename: "pool/libc6_2.36.deb".into(),
                    sha256: "cccc".into(),
                },
            ],
        );
        by_name.insert(
            "libgcc-s1".into(),
            vec![PackageRecord {
                version: "12.0".into(),
                depends: String::new(),
                predepends: String::new(),
                filename: "pool/libgcc-s1_12.0.deb".into(),
                sha256: "dddd".into(),
            }],
        );
        Index { by_name, provides: HashMap::new() }
    }

    #[test]
    fn parse_atom_with_constraint() {
        let alts = parse_alternatives("libc6 (>= 2.34)");
        assert_eq!(alts.len(), 1);
        assert_eq!(alts[0].name, "libc6");
        assert_eq!(alts[0].op, Some(VerOp::Ge));
        assert_eq!(alts[0].ver.as_deref(), Some("2.34"));
    }

    #[test]
    fn parse_alternatives_multi() {
        let alts = parse_alternatives("default-mta | mail-transport-agent");
        assert_eq!(alts.len(), 2);
        assert_eq!(alts[0].name, "default-mta");
        assert_eq!(alts[1].name, "mail-transport-agent");
    }

    #[test]
    fn picks_max_satisfying_version() {
        let idx = mk_index();
        let rec = pick_version(&idx, "libc6", Some(VerOp::Ge), Some("2.34")).unwrap();
        assert_eq!(rec.version, "2.36"); // 满足约束中选最大
    }

    #[test]
    fn resolves_transitive_closure() {
        let idx = mk_index();
        let template = vec![Constraint::unconstrained("ripgrep")];
        let res = resolve(&idx, &template, &HashMap::new());
        // 闭包应含 ripgrep + 两个传递依赖
        assert!(res.closure.contains_key("ripgrep"));
        assert!(res.closure.contains_key("libc6"));
        assert!(res.closure.contains_key("libgcc-s1"));
        assert_eq!(res.diagnostics.unresolved.len(), 0);
    }

    #[test]
    fn closure_id_deterministic() {
        // PoC-3 核心:同输入三次求解,closure_id 必须一致。
        let idx = mk_index();
        let template = vec![Constraint::unconstrained("ripgrep")];
        let id1 = build_lock(resolve(&idx, &template, &HashMap::new())).closure_id;
        let id2 = build_lock(resolve(&idx, &template, &HashMap::new())).closure_id;
        let id3 = build_lock(resolve(&idx, &template, &HashMap::new())).closure_id;
        assert_eq!(id1, id2);
        assert_eq!(id2, id3);
        assert!(id1.starts_with("clo-"));
    }

    #[test]
    fn exclude_override() {
        let idx = mk_index();
        let template = vec![Constraint::unconstrained("ripgrep")];
        let mut ov = HashMap::new();
        ov.insert("libgcc-s1".to_string(), Override::Exclude);
        let res = resolve(&idx, &template, &ov);
        assert!(!res.closure.contains_key("libgcc-s1"));
        assert!(res.diagnostics.excluded_hit.contains(&"libgcc-s1".to_string()));
    }

    #[test]
    fn detects_version_conflict() {
        // 两个互斥的顶层约束:libc6 (>= 2.36) 与 libc6 (<< 2.35)。
        // 无单一版本同时满足 → 应报 1 条冲突(ai/02 触发 repair 的依据),不再静默吞。
        let idx = mk_index();
        let template = vec![
            Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.36".into()) },
            Constraint { name: "libc6".into(), op: Some(VerOp::Lt), ver: Some("2.35".into()) },
        ];
        let res = resolve(&idx, &template, &HashMap::new());
        assert_eq!(res.diagnostics.conflicts.len(), 1, "应检出 1 条版本冲突: {:?}", res.diagnostics.conflicts);
        let c = &res.diagnostics.conflicts[0];
        assert_eq!(c.package, "libc6");
        // closure 保留先到约束选定的版本(确定性);冲突约束被记录而非覆盖。
        assert!(res.closure.contains_key("libc6"), "冲突不应让包从闭包消失");
    }

    #[test]
    fn no_conflict_when_constraints_compatible() {
        // 两个兼容约束:libc6 (>= 2.34) 与 libc6 (>= 2.36) → 2.36 同时满足,无冲突。
        let idx = mk_index();
        let template = vec![
            Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.34".into()) },
            Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.36".into()) },
        ];
        let res = resolve(&idx, &template, &HashMap::new());
        assert!(res.diagnostics.conflicts.is_empty(), "兼容约束不应报冲突: {:?}", res.diagnostics.conflicts);
        assert_eq!(res.closure.get("libc6").unwrap().version, "2.36");
    }

    #[test]
    fn conflict_records_source() {
        // 顶层要 libc6 (<< 2.35),先选 2.34;再有顶层 libc6 (>= 2.36) 不满足 → 冲突,来源 <template>。
        let idx = mk_index();
        let template = vec![
            Constraint { name: "libc6".into(), op: Some(VerOp::Lt), ver: Some("2.35".into()) },
            Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.36".into()) },
        ];
        let res = resolve(&idx, &template, &HashMap::new());
        assert_eq!(res.diagnostics.conflicts.len(), 1);
        assert_eq!(res.diagnostics.conflicts[0].source, "<template>");
    }

    #[test]
    fn repair_suggestion_finds_satisfying_version() {
        // 冲突约束 >=2.34 与 <=2.36:index 有 2.36 同时满足两者 → 方案A 建议放宽到 2.36。
        // (注:>=2.34 与 <=2.36 本身兼容,但若先选 2.34 再来 <=... 不冲突;这里用一对会触发冲突
        //  又确实存在交集的约束。改用 >=2.36 与 <=2.36 必然先选其一再冲突,交集恰为 2.36。)
        let idx = mk_index();
        let template = vec![
            Constraint { name: "libc6".into(), op: Some(VerOp::Le), ver: Some("2.34".into()) },
            Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.36".into()) },
        ];
        let res = resolve(&idx, &template, &HashMap::new());
        // <=2.34 选 2.34;>=2.36 不满足 → 冲突。
        assert_eq!(res.diagnostics.conflicts.len(), 1);
        // 方案A:无单一版本同时满足 <=2.34 与 >=2.36(2.34 与 2.36 之间无候选)→ None。
        let sug = res.diagnostics.repair_suggestions.iter().find(|s| s.package == "libc6").unwrap();
        assert_eq!(sug.satisfying_version, None, "<=2.34 与 >=2.36 无单一共存版本");
        assert_eq!(sug.constraints.len(), 2, "应收集两条约束");
    }

    #[test]
    fn repair_suggestion_some_when_intersection_exists() {
        // 直接测纯函数 find_satisfying_version:>=2.34 与 <=2.36 → 2.36(两候选都满足,取最高)。
        let idx = mk_index();
        let v = find_satisfying_version(
            &idx,
            "libc6",
            &[(VerOp::Ge, "2.34".into()), (VerOp::Le, "2.36".into())],
        );
        assert_eq!(v, Some("2.36".to_string()), "2.34 与 2.36 都满足,取最高 2.36");
    }

    #[test]
    fn repair_suggestion_none_when_no_intersection() {
        // 纯函数:>=2.36 与 <<2.35 无单一版本同时满足 → None。
        let idx = mk_index();
        let v = find_satisfying_version(
            &idx,
            "libc6",
            &[(VerOp::Ge, "2.36".into()), (VerOp::Lt, "2.35".into())],
        );
        assert_eq!(v, None);
    }

    /// 三版本 libc6 的 index(2.34/2.35/2.36),供 repair 自动应用测试:
    /// 约束 >=2.35 与 <=2.35 互斥(各选 2.36/2.34)但交集恰为 2.35。
    fn mk_index_three() -> Index {
        let mut by_name: HashMap<String, Vec<PackageRecord>> = HashMap::new();
        let mk = |v: &str| PackageRecord {
            version: v.into(),
            depends: String::new(),
            predepends: String::new(),
            filename: format!("pool/libc6_{v}.deb"),
            sha256: v.into(),
        };
        by_name.insert("libc6".into(), vec![mk("2.34"), mk("2.35"), mk("2.36")]);
        Index { by_name, provides: HashMap::new() }
    }

    #[test]
    fn repair_auto_applies_to_coexisting_version() {
        // >=2.35 与 <=2.35 冲突(各选 2.36/2.34),交集 2.35 → 自动钉到 2.35,无冲突。
        let idx = mk_index_three();
        let template = vec![
            Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.35".into()) },
            Constraint { name: "libc6".into(), op: Some(VerOp::Le), ver: Some("2.35".into()) },
        ];
        let repaired = resolve_with_repair(&idx, &template, &HashMap::new());
        assert!(
            repaired.resolution.diagnostics.conflicts.is_empty(),
            "自动 repair 后应无冲突: {:?}", repaired.resolution.diagnostics.conflicts
        );
        assert_eq!(repaired.applied.len(), 1, "应应用 1 次放宽");
        assert_eq!(repaired.applied[0].package, "libc6");
        assert_eq!(repaired.applied[0].pinned_version, "2.35");
        assert!(repaired.unresolved_conflicts.is_empty());
        assert_eq!(repaired.resolution.closure.get("libc6").unwrap().version, "2.35");
    }

    #[test]
    fn repair_leaves_unresolved_when_no_coexisting_version() {
        // >=2.36 与 <=2.34 无交集(2.34 与 2.36 间无候选)→ 方案A 无能为力,标记未解决。
        let idx = mk_index_three();
        let template = vec![
            Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.36".into()) },
            Constraint { name: "libc6".into(), op: Some(VerOp::Le), ver: Some("2.34".into()) },
        ];
        let repaired = resolve_with_repair(&idx, &template, &HashMap::new());
        assert!(repaired.applied.is_empty(), "无交集不应应用放宽");
        assert_eq!(repaired.unresolved_conflicts, vec!["libc6".to_string()]);
        assert!(!repaired.resolution.diagnostics.conflicts.is_empty(), "冲突仍在");
    }

    #[test]
    fn repair_noop_when_no_conflict() {
        // 无冲突时 resolve_with_repair 等价于 resolve,不应用任何放宽。
        let idx = mk_index();
        let template = vec![Constraint::unconstrained("ripgrep")];
        let repaired = resolve_with_repair(&idx, &template, &HashMap::new());
        assert!(repaired.applied.is_empty());
        assert!(repaired.unresolved_conflicts.is_empty());
        assert!(repaired.resolution.closure.contains_key("ripgrep"));
    }

    /// index:openssl 仅 3.0/3.2(无中间版本,方案A 对 <<3.1 与 >=3.2 必无解);
    /// app-x 有 1.0(依赖 openssl <<3.1)与 1.2(依赖放宽 openssl >=3.0);app-y 1.0 依赖 openssl >=3.2。
    fn mk_index_parent_upgrade() -> Index {
        let mut by_name: HashMap<String, Vec<PackageRecord>> = HashMap::new();
        let p = |v: &str, deps: &str| PackageRecord {
            version: v.into(),
            depends: deps.into(),
            predepends: String::new(),
            filename: format!("pool/{v}.deb"),
            sha256: v.into(),
        };
        by_name.insert("openssl".into(), vec![p("3.0", ""), p("3.2", "")]);
        by_name.insert("app-x".into(), vec![p("1.0", "openssl (<< 3.1)"), p("1.2", "openssl (>= 3.0)")]);
        by_name.insert("app-y".into(), vec![p("1.0", "openssl (>= 3.2)")]);
        Index { by_name, provides: HashMap::new() }
    }

    #[test]
    fn extract_constraint_for_parses_dep() {
        assert_eq!(
            extract_constraint_for("libc6 (>= 2.34), openssl (<< 3.1)", "openssl"),
            Some((VerOp::Lt, "3.1".to_string()))
        );
        // 裸依赖(无版本)→ None。
        assert_eq!(extract_constraint_for("libc6, openssl", "openssl"), None);
        // 不含该依赖 → None。
        assert_eq!(extract_constraint_for("libc6 (>= 2.34)", "openssl"), None);
    }

    #[test]
    fn repair_b_suggests_parent_upgrade() {
        // app-x 当前被钉在 1.0(顶层 <<1.1),其依赖 openssl<<3.1;app-y 要 openssl>=3.2
        // → openssl 无共存版本(方案A 失败)。方案B:若把 app-x 升到 1.2(放宽为 openssl>=3.0),
        //   openssl 可取 3.2 共存 → 建议升级 app-x。
        let idx = mk_index_parent_upgrade();
        let template = vec![
            Constraint { name: "app-x".into(), op: Some(VerOp::Lt), ver: Some("1.1".into()) },
            Constraint::unconstrained("app-y"),
        ];
        let res = resolve(&idx, &template, &HashMap::new());
        // openssl 应有冲突且方案A 无解。
        assert!(res.diagnostics.conflicts.iter().any(|c| c.package == "openssl"), "openssl 应冲突: {:?}", res.diagnostics.conflicts);
        let a = res.diagnostics.repair_suggestions.iter().find(|s| s.package == "openssl").unwrap();
        assert_eq!(a.satisfying_version, None, "方案A 应无解(openssl 无 3.1~3.2 间版本)");
        // 方案B 应建议升级 app-x 到 1.2,openssl 取 3.2。
        let b = res.diagnostics.repair_suggestions_b.iter().find(|s| s.dependency == "openssl")
            .expect("应有方案B 建议");
        assert_eq!(b.parent, "app-x");
        assert_eq!(b.upgrade_parent_to, "1.2");
        assert_eq!(b.dependency_version, "3.2");
    }

    #[test]
    fn suggest_upgrade_parent_pure_fn() {
        // 纯函数:openssl 被 app-x(<<3.1)与 app-y(>=3.2,作 other)约束;
        // 升级 app-x 到 1.2(>=3.0)→ openssl 可取 3.2。
        let idx = mk_index_parent_upgrade();
        let b = suggest_upgrade_parent(
            &idx,
            "openssl",
            &[("app-x".to_string(), (VerOp::Lt, "3.1".to_string()))],
            &[(VerOp::Ge, "3.2".to_string())],
        );
        let b = b.expect("应找到升级 app-x 的方案");
        assert_eq!(b.parent, "app-x");
        assert_eq!(b.upgrade_parent_to, "1.2");
        assert_eq!(b.dependency_version, "3.2");
    }

    #[test]
    fn repair_c_keeps_two_when_each_side_satisfiable() {
        // 顶层互斥 libc6 (>=2.36) 与 (<=2.34):A 无单一共存版本、B 无父包可升,
        // 但 >=2.36 可用 2.36、<=2.34 可用 2.34 → 方案C 保留两份(各方各用各版本)。
        let idx = mk_index(); // libc6 仅 2.34/2.36
        let template = vec![
            Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.36".into()) },
            Constraint { name: "libc6".into(), op: Some(VerOp::Le), ver: Some("2.34".into()) },
        ];
        let res = resolve(&idx, &template, &HashMap::new());
        // A 无解、B 无建议、C 兜底(纯版本冲突两方各自可满足时,保留两份总能兜住)。
        assert!(res.diagnostics.repair_suggestions.iter().any(|s| s.package == "libc6" && s.satisfying_version.is_none()));
        assert!(res.diagnostics.repair_suggestions_b.is_empty(), "无父包可升,B 应空");
        let c = res.diagnostics.keep_two_suggestions.iter().find(|s| s.package == "libc6").expect("应有方案C 保留两份建议");
        // 两份:2.34(给 <=2.34 一方)与 2.36(给 >=2.36 一方)。
        assert_eq!(c.version_a, "2.34");
        assert_eq!(c.version_b, "2.36");
        assert!(res.diagnostics.unrepairable.is_empty(), "C 能兜底则不落 D");
    }

    #[test]
    fn suggest_keep_two_pure_fn() {
        let idx = mk_index();
        let c = suggest_keep_two(
            &idx,
            "libc6",
            &[
                (VerOp::Ge, "2.36".into(), "app-new".into()),
                (VerOp::Le, "2.34".into(), "app-old".into()),
            ],
        ).expect("应能拆两份");
        assert_eq!(c.version_a, "2.34");
        assert_eq!(c.sources_a, vec!["app-old".to_string()]);
        assert_eq!(c.version_b, "2.36");
        assert_eq!(c.sources_b, vec!["app-new".to_string()]);
    }

    #[test]
    fn suggest_keep_two_none_when_all_same_version() {
        // 两约束都满足同一版本(2.36)→ 非"保留两份"场景(A 本应已解)。
        let idx = mk_index();
        let c = suggest_keep_two(
            &idx,
            "libc6",
            &[
                (VerOp::Ge, "2.36".into(), "a".into()),
                (VerOp::Ge, "2.34".into(), "b".into()), // 也选 2.36(最高满足)
            ],
        );
        assert!(c.is_none(), "同版本不应建议保留两份");
    }

    #[test]
    fn repair_c_not_raised_when_b_solves() {
        // 方案B 有解时,不应升级到方案C/D。
        let idx = mk_index_parent_upgrade();
        let template = vec![
            Constraint { name: "app-x".into(), op: Some(VerOp::Lt), ver: Some("1.1".into()) },
            Constraint::unconstrained("app-y"),
        ];
        let res = resolve(&idx, &template, &HashMap::new());
        assert!(res.diagnostics.repair_suggestions_b.iter().any(|s| s.dependency == "openssl"));
        assert!(res.diagnostics.keep_two_suggestions.is_empty(), "B 有解时不应升级到方案C");
        assert!(res.diagnostics.unrepairable.is_empty(), "B 有解时不应升级到方案D");
    }

    #[test]
    fn parse_packages_basic() {
        // Debian Packages 段落格式:两个包,含 Provides 虚包。
        let text = "\
Package: ripgrep
Version: 13.0.0-1
Depends: libc6 (>= 2.34), libgcc-s1 (>= 4.2)
Filename: pool/r/ripgrep_13.0.0-1_amd64.deb
SHA256: aabbcc

Package: libc6
Version: 2.36-1
Provides: libc-dev, glibc (= 2.36)
Filename: pool/g/glibc/libc6_2.36-1_amd64.deb
SHA256: ddeeff
";
        let idx = Index::from_packages_str(text);
        assert_eq!(idx.package_count(), 2);
        // 字段提取正确
        let rg = &idx.by_name["ripgrep"][0];
        assert_eq!(rg.version, "13.0.0-1");
        assert!(rg.depends.contains("libc6"));
        assert_eq!(rg.sha256, "aabbcc");
        // Provides 虚包映射:libc-dev / glibc → libc6
        assert_eq!(idx.provides["libc-dev"], vec!["libc6".to_string()]);
        assert_eq!(idx.provides["glibc"], vec!["libc6".to_string()]); // 去掉了 (= 2.36)
    }

    #[test]
    fn parse_packages_skips_continuation_and_blank_tail() {
        // 续行跳过 + 末段无尾随空行也能 flush。
        let text = "\
Package: foo
Version: 1.0
Description: a package
 with a continuation line
 and another
Depends: bar";
        let idx = Index::from_packages_str(text);
        assert_eq!(idx.package_count(), 1);
        let foo = &idx.by_name["foo"][0];
        assert_eq!(foo.version, "1.0");
        assert_eq!(foo.depends, "bar"); // 续行未污染 depends
    }

    #[test]
    fn parse_then_resolve_deterministic() {
        // 端到端(小真数据形态):解析 → 求解 → closure_id 两次一致(PoC-3 可复现铁律)。
        let text = "\
Package: app
Version: 1.0
Depends: liba

Package: liba
Version: 2.0
Depends: libc

Package: libc
Version: 2.36
";
        let idx = Index::from_packages_str(text);
        let template = vec![Constraint::unconstrained("app")];
        let id1 = build_lock(resolve(&idx, &template, &HashMap::new())).closure_id;
        let id2 = build_lock(resolve(&idx, &template, &HashMap::new())).closure_id;
        assert_eq!(id1, id2, "同输入两次求解 closure_id 必须一致");
        // 传递闭包:app→liba→libc 全解出
        let res = resolve(&idx, &template, &HashMap::new());
        assert!(res.closure.contains_key("app"));
        assert!(res.closure.contains_key("liba"));
        assert!(res.closure.contains_key("libc"));
        assert_eq!(res.diagnostics.unresolved.len(), 0);
    }
}
