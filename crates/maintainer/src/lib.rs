//! AI 维护者安全闸门:**verify**——拦在 candidate → verified 之间的独立机器判定。
//!
//! ADR-0003 边界1 限制"AI 不直接选 hash",但 AI 仍能通过**产出偏向旧版本的约束**
//! (如 `==3.11.7`,该版含已知 CVE)间接达成危险结果——旧版有合法签名、过完整性/闭合/层校验,
//! 于是悄悄进入 verified。更危险的是"是否需人确认"此前由 AI 自述(`needs_user_confirm`),
//! AI 误标/恶意标"无需确认"即可绕过人类否决,形成循环论证。
//!
//! 本 crate 把危险判定从"AI 自述"改为"verify 阶段的确定性、独立检查",切断
//! "AI 既提议、又自评是否危险、还自己放行"的闭环。设计见
//! `docs/architecture/runtime/01-generation-lifecycle.md` §3 + §3.1、`docs/ai/01-maintainer-loop.md`。
//!
//! # 五条判据(lifecycle §3)
//! 1. **完整性**:closure 内每个 store 对象重算哈希校验(复用 [`aevum_store::Store::get`])。
//! 2. **闭合性**:candidate lock 内每包的 `Depends` 都能在 closure(或 foundation 提供集)内满足。
//! 3. **Layer 约束**:required foundation 包在场且版本精确;软件层变更未触 foundation。
//!    —— **本轮未实现**(无 foundation manifest 解析),`foundation_violations` 恒空,见 [`verify`] 待办。
//! 4. **安全/版本回退**:
//!    - ① CVE 命中 —— **本轮未实现**(需外部 CVE 库),见待办。
//!    - ② 某包版本【低于】当前 active 同名包 → 标记"版本回退",**强制人工确认**。
//! 5. `needs_user_confirm` 由 verify **独立判定**,不信任 AI 自述:版本回退/CVE 命中即强制 `true`。
//!
//! # 数据流(关键,易踩坑)
//! - **判据1** 的输入是 candidate **世代**的 store 对象列表(`<hash>-<name>`,12-hex 内容哈希),
//!   **不是** lock 的 `fingerprint`——后者是 `.deb` 整包的 SHA256(下载校验用),语义不同。
//! - **判据2/4** 的输入是 candidate **lock**(`name@version#fingerprint`),有版本语义,数据完备。
//! - 故 [`verify`] 同时接收 lock(语义)与 object_ids(store 哈希),分别喂两类判据。
//!
//! 纯确定性、无 AI、无随机、无时钟、无网络——对齐 solver/store 的可测风格。

use aevum_solver::version::deb_ver_cmp;
use aevum_solver::{parse_alternatives, Index, Lock};
use aevum_store::Store;
use std::cmp::Ordering;
use std::collections::HashSet;

pub mod foundation;
pub use foundation::{FoundationManifest, FoundationPackage, ManifestError};

/// 判据1:某 store 对象完整性校验失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrityFailure {
    /// 世代引用的 store 对象目录名(`<hash>-<name>`)。
    pub object_id: String,
    /// 失败原因(对象缺失 / 内容哈希失配 / 目录名不合法)。
    pub reason: String,
}

/// 判据2:某包的某条依赖在 closure 内无法满足。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnclosedDep {
    /// 提出该依赖的包名。
    pub package: String,
    /// 未满足的依赖原子原文(可能是 `a | b` alternatives)。
    pub requirement: String,
}

/// 判据4②:某包版本低于当前 active 同名包(版本回退)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRollback {
    pub package: String,
    /// candidate 中的版本(更低)。
    pub candidate_version: String,
    /// active 中的版本(更高)。
    pub active_version: String,
}

/// verify 的判定报告。`passed` 与 `needs_user_confirm` 是两个**独立**维度:
/// - `passed=false`:硬性校验失败(完整性/闭合/层),世代 → failed,不可激活。
/// - `needs_user_confirm=true`:校验通过但触发安全判据(版本回退/CVE),**强制人工确认**才能激活。
///
/// 二者都为"安全"时(`passed && !needs_user_confirm`)才可自动 verified。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub passed: bool,
    pub integrity_failures: Vec<IntegrityFailure>,
    pub unclosed_deps: Vec<UnclosedDep>,
    /// 判据3:本轮恒空(无 foundation manifest 解析,见 [`verify`] 待办)。
    pub foundation_violations: Vec<String>,
    pub version_rollbacks: Vec<VersionRollback>,
    /// **机器独立判定**,不信任 AI 自述。版本回退/CVE 命中即强制 `true`。
    pub needs_user_confirm: bool,
}

impl VerifyReport {
    /// 可自动激活当且仅当:硬性校验全过且无需人工确认。
    pub fn auto_activatable(&self) -> bool {
        self.passed && !self.needs_user_confirm
    }
}

/// 安全闸门主入口。**输入**:
/// - `candidate_lock`:待验证世代的 lock(版本语义,判据2/4)。
/// - `active_lock`:当前 active 世代的 lock,用于版本回退比较;首次安装传 `None`。
/// - `index`:求解时同一份包索引快照,用于查每包 `Depends` 做闭合性(判据2)。
/// - `store`:内容寻址 store,用于完整性重算(判据1)。
/// - `object_ids`:candidate 世代引用的 store 对象列表(`<hash>-<name>`,判据1)。
/// - `foundation_provided`:foundation 层提供的包名集合,闭合性判定时视为"已满足"。
///   本轮 CLI 传空切片(无 foundation manifest);留参数以备接入,不致 foundation 依赖被误报。
///
/// # 待办(诚实标注)
/// - **判据4①**(CVE 命中):需外部 CVE 库,本轮未实现。
/// - **闭合性边界**:foundation 提供的依赖由 `foundation_provided` 显式喂入或 `foundation` manifest 自动并入;
///   二者都未覆盖的 foundation 依赖会被误报为 unclosed。
pub fn verify(
    candidate_lock: &Lock,
    active_lock: Option<&Lock>,
    index: &Index,
    store: &Store,
    object_ids: &[String],
    foundation_provided: &[String],
    foundation: Option<&FoundationManifest>,
) -> VerifyReport {
    let mut report = VerifyReport::default();

    // ---- 判据1:完整性(store 对象逐个重算哈希)----
    for object_id in object_ids {
        // 对象目录名格式 `<hash>-<name>`;hash 为 12-hex(不含 '-'),从首个 '-' 拆。
        match object_id.split_once('-') {
            Some((hash, name)) if !hash.is_empty() && !name.is_empty() => {
                if let Err(e) = store.get(hash, name) {
                    report.integrity_failures.push(IntegrityFailure {
                        object_id: object_id.clone(),
                        reason: e.to_string(),
                    });
                }
            }
            _ => report.integrity_failures.push(IntegrityFailure {
                object_id: object_id.clone(),
                reason: "对象目录名不合法(期望 <hash>-<name>)".into(),
            }),
        }
    }

    // ---- 判据2:闭合性(每包 Depends 都能在 closure / foundation 提供集内满足)----
    // closure 集合 = candidate lock 里所有包名。
    let closure: HashSet<&str> = candidate_lock.locked.iter().map(|p| p.name.as_str()).collect();
    // foundation 提供集 = 显式 foundation_provided + manifest 声明的全部包名(消除闭合性误报)。
    let mut provided: HashSet<&str> = foundation_provided.iter().map(|s| s.as_str()).collect();
    if let Some(fm) = foundation {
        for p in &fm.packages {
            provided.insert(p.name.as_str());
        }
    }

    for pkg in &candidate_lock.locked {
        // 从 index 找该包"被锁定的那个版本"的记录,取其 Depends/Pre-Depends。
        let Some(rec) = index
            .by_name
            .get(&pkg.name)
            .and_then(|recs| recs.iter().find(|r| r.version == pkg.version))
        else {
            // index 里找不到该 name@version——求解所用索引与 verify 索引不一致,记为闭合性问题。
            report.unclosed_deps.push(UnclosedDep {
                package: pkg.name.clone(),
                requirement: format!("<index 缺记录: {}@{}>", pkg.name, pkg.version),
            });
            continue;
        };

        // Depends 与 Pre-Depends 都是"必须满足"的运行时/安装期依赖,逗号分隔的原子。
        for field in [rec.depends.as_str(), rec.predepends.as_str()] {
            for atom in field.split(',') {
                let atom = atom.trim();
                if atom.is_empty() {
                    continue;
                }
                // 一个原子内 `a | b` 任一满足即可。
                let alts = parse_alternatives(atom);
                if alts.is_empty() {
                    // 解析不出合法约束(罕见,如纯架构限定)——保守跳过,不误报。
                    continue;
                }
                let satisfied = alts.iter().any(|c| {
                    closure.contains(c.name.as_str())
                        || provided.contains(c.name.as_str())
                        // 虚包:closure 内某真实包 provides 它。
                        || index
                            .provides
                            .get(&c.name)
                            .map(|reals| reals.iter().any(|r| closure.contains(r.as_str())))
                            .unwrap_or(false)
                });
                if !satisfied {
                    report.unclosed_deps.push(UnclosedDep {
                        package: pkg.name.clone(),
                        requirement: atom.to_string(),
                    });
                }
            }
        }
    }

    // ---- 判据3:Layer 约束(foundation manifest 在场时校验,见 layers/01 §4)----
    // ① 所有 required foundation 包必须在 candidate 闭包内(不能删核心组件)。
    // ② 在场的 foundation 包版本必须与 manifest 精确匹配(不能降级/篡改核心包版本)。
    // 无 manifest 时跳过(恒空,行为同旧版,保持向后兼容)。
    if let Some(fm) = foundation {
        // candidate 包名 → 版本。
        let cand_vers: std::collections::HashMap<&str, &str> = candidate_lock
            .locked
            .iter()
            .map(|p| (p.name.as_str(), p.version.as_str()))
            .collect();
        for fp in &fm.packages {
            match cand_vers.get(fp.name.as_str()) {
                None => {
                    if fp.required {
                        report.foundation_violations.push(format!(
                            "缺核心组件: {}(required,manifest 要求 {})",
                            fp.name, fp.version
                        ));
                    }
                }
                Some(&cand_ver) => {
                    // ② 版本必须精确匹配(foundation 包是硬锁,见 §4.2)。
                    if cand_ver != fp.version {
                        report.foundation_violations.push(format!(
                            "核心包 {} 版本必须是 {}(候选为 {})",
                            fp.name, fp.version, cand_ver
                        ));
                    }
                }
            }
        }
    }

    // ---- 判据4②:版本回退(candidate 某包低于 active 同名包)----
    if let Some(active) = active_lock {
        // active 包名 → 版本,便于查同名。
        let active_vers: std::collections::HashMap<&str, &str> = active
            .locked
            .iter()
            .map(|p| (p.name.as_str(), p.version.as_str()))
            .collect();
        for pkg in &candidate_lock.locked {
            if let Some(&active_ver) = active_vers.get(pkg.name.as_str()) {
                if deb_ver_cmp(&pkg.version, active_ver) == Ordering::Less {
                    report.version_rollbacks.push(VersionRollback {
                        package: pkg.name.clone(),
                        candidate_version: pkg.version.clone(),
                        active_version: active_ver.to_string(),
                    });
                }
            }
        }
    }

    // ---- 判据4①:CVE 命中 —— 本轮未实现(需外部 CVE 库)----

    // ---- 汇总 ----
    // 硬性校验:完整性 + 闭合性 + 层约束全过才 passed(层约束本轮恒空 → 不阻断)。
    report.passed = report.integrity_failures.is_empty()
        && report.unclosed_deps.is_empty()
        && report.foundation_violations.is_empty();
    // 安全判据由 verify 机器独立判定,不信任 AI 自述:版本回退命中即强制人工确认。
    report.needs_user_confirm = !report.version_rollbacks.is_empty();

    report
}

#[cfg(test)]
mod tests;
