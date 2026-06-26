//! verify 安全闸门单测。
//!
//! 跨平台可测:闭合性、版本回退、对象缺失、报告汇总逻辑(纯数据)。
//! unix 专有:完整性"哈希失配"重算(`Store::get` 的内容校验 `#[cfg(unix)]`)。

use super::*;
use aevum_solver::{Diagnostics, LockedPackage, PackageRecord};
use aevum_store::{FileMeta, Store};
use std::collections::HashMap;

/// 临时目录(进程级唯一,避免并发测试串扰)。
fn tmp(tag: &str) -> std::path::PathBuf {
    let base = std::env::temp_dir().join("aevum-maintainer-test");
    let _ = std::fs::create_dir_all(&base);
    base.join(format!("{tag}-{}", std::process::id()))
}

fn lock(closure_id: &str, pkgs: &[(&str, &str)]) -> Lock {
    let locked: Vec<LockedPackage> = pkgs
        .iter()
        .map(|(n, v)| LockedPackage {
            name: (*n).to_string(),
            version: (*v).to_string(),
            fingerprint: format!("sha256:{n}-{v}"),
            filename: format!("pool/{n}_{v}.deb"),
        })
        .collect();
    Lock {
        closure_id: closure_id.to_string(),
        package_count: locked.len(),
        locked,
        diagnostics: Diagnostics::default(),
    }
}

/// 构造 index:每包给定 (version, depends)。
fn index(pkgs: &[(&str, &str, &str)]) -> Index {
    let mut by_name: HashMap<String, Vec<PackageRecord>> = HashMap::new();
    for (name, ver, depends) in pkgs {
        by_name.entry((*name).to_string()).or_default().push(PackageRecord {
            version: (*ver).to_string(),
            depends: (*depends).to_string(),
            predepends: String::new(),
            filename: format!("pool/{name}.deb"),
            sha256: String::new(),
        });
    }
    Index { by_name, provides: HashMap::new() }
}

// ---- 判据1:完整性 ----

#[test]
fn integrity_passes_for_real_objects() {
    let store = Store::open(tmp("integ-ok")).unwrap();
    // 入库两个真实对象,用返回目录名作 object_id。
    let meta = FileMeta { mode: 0o644, is_symlink: false };
    let d1 = store.put("hello", b"hello-body", meta).unwrap();
    let d2 = store.put("world", b"world-body", meta).unwrap();
    let ids: Vec<String> = [d1, d2]
        .iter()
        .map(|d| d.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    let cand = lock("clo-a", &[]);
    let idx = index(&[]);
    let report = verify(&cand, None, &idx, &store, &ids, &[], None);

    assert!(report.integrity_failures.is_empty(), "{:?}", report.integrity_failures);
    assert!(report.passed);
    assert!(report.auto_activatable());
}

#[test]
fn integrity_fails_for_missing_object() {
    let store = Store::open(tmp("integ-missing")).unwrap();
    // 一个不存在的 object_id(合法格式但 store 里没有)。
    let ids = vec!["deadbeef0000-ghost".to_string()];

    let cand = lock("clo-b", &[]);
    let idx = index(&[]);
    let report = verify(&cand, None, &idx, &store, &ids, &[], None);

    assert_eq!(report.integrity_failures.len(), 1);
    assert_eq!(report.integrity_failures[0].object_id, "deadbeef0000-ghost");
    assert!(!report.passed, "缺对象必须使硬性校验失败");
}

#[test]
fn integrity_fails_for_malformed_object_id() {
    let store = Store::open(tmp("integ-malformed")).unwrap();
    // 无 '-' 分隔,不合法。
    let ids = vec!["noseparator".to_string()];

    let report = verify(&lock("clo-c", &[]), None, &index(&[]), &store, &ids, &[], None);
    assert_eq!(report.integrity_failures.len(), 1);
    assert!(report.integrity_failures[0].reason.contains("不合法"));
    assert!(!report.passed);
}

#[cfg(unix)]
#[test]
fn integrity_fails_on_content_tamper() {
    // unix-only:get 重算内容哈希,篡改后失配。
    let store = Store::open(tmp("integ-tamper")).unwrap();
    let meta = FileMeta { mode: 0o644, is_symlink: false };
    let dir = store.put("victim", b"original", meta).unwrap();
    let id = dir.file_name().unwrap().to_string_lossy().into_owned();
    // 篡改对象内文件内容(hash 仍是旧的,重算必失配)。
    std::fs::write(dir.join("victim"), b"tampered!!").unwrap();

    let report = verify(&lock("clo-d", &[]), None, &index(&[]), &store, &[id], &[], None);
    assert_eq!(report.integrity_failures.len(), 1, "篡改应被完整性校验抓到");
    assert!(!report.passed);
}

// ---- 判据2:闭合性 ----

#[test]
fn closure_passes_when_all_deps_present() {
    // ripgrep 依赖 libc6,二者都在 lock 内 → 闭合。
    let store = Store::open(tmp("clo-ok")).unwrap();
    let idx = index(&[
        ("ripgrep", "13.0.0", "libc6 (>= 2.34)"),
        ("libc6", "2.34", ""),
    ]);
    let cand = lock("clo-e", &[("ripgrep", "13.0.0"), ("libc6", "2.34")]);

    let report = verify(&cand, None, &idx, &store, &[], &[], None);
    assert!(report.unclosed_deps.is_empty(), "{:?}", report.unclosed_deps);
    assert!(report.passed);
}

#[test]
fn closure_fails_on_missing_dep() {
    // ripgrep 依赖 libc6,但 lock 里没有 libc6 → 不闭合。
    let store = Store::open(tmp("clo-miss")).unwrap();
    let idx = index(&[("ripgrep", "13.0.0", "libc6 (>= 2.34)")]);
    let cand = lock("clo-f", &[("ripgrep", "13.0.0")]);

    let report = verify(&cand, None, &idx, &store, &[], &[], None);
    assert_eq!(report.unclosed_deps.len(), 1);
    assert_eq!(report.unclosed_deps[0].package, "ripgrep");
    assert!(report.unclosed_deps[0].requirement.contains("libc6"));
    assert!(!report.passed, "缺依赖必须使硬性校验失败");
}

#[test]
fn closure_satisfied_by_foundation_provided() {
    // libc6 不在 lock,但由 foundation 提供 → 视为已满足,闭合通过。
    let store = Store::open(tmp("clo-found")).unwrap();
    let idx = index(&[("ripgrep", "13.0.0", "libc6 (>= 2.34)")]);
    let cand = lock("clo-g", &[("ripgrep", "13.0.0")]);

    let report = verify(&cand, None, &idx, &store, &[], &["libc6".to_string()], None);
    assert!(report.unclosed_deps.is_empty(), "foundation 提供的依赖不应被误报");
    assert!(report.passed);
}

#[test]
fn closure_alternatives_one_present_is_ok() {
    // app 依赖 `liba | libb`,closure 内有 libb → 满足。
    let store = Store::open(tmp("clo-alt")).unwrap();
    let idx = index(&[
        ("app", "1.0", "liba | libb"),
        ("libb", "2.0", ""),
    ]);
    let cand = lock("clo-h", &[("app", "1.0"), ("libb", "2.0")]);

    let report = verify(&cand, None, &idx, &store, &[], &[], None);
    assert!(report.unclosed_deps.is_empty(), "alternatives 任一满足即闭合: {:?}", report.unclosed_deps);
}

// ---- 判据4②:版本回退 ----

#[test]
fn version_rollback_detected_and_forces_confirm() {
    // candidate 把 openssl 从 3.0.2 降到 3.0.1 → 回退,强制人工确认。
    let store = Store::open(tmp("rollback")).unwrap();
    let idx = index(&[
        ("openssl", "3.0.1", ""),
        ("openssl", "3.0.2", ""),
    ]);
    let active = lock("clo-active", &[("openssl", "3.0.2")]);
    let cand = lock("clo-cand", &[("openssl", "3.0.1")]);

    let report = verify(&cand, Some(&active), &idx, &store, &[], &[], None);
    assert_eq!(report.version_rollbacks.len(), 1);
    assert_eq!(report.version_rollbacks[0].package, "openssl");
    assert_eq!(report.version_rollbacks[0].candidate_version, "3.0.1");
    assert_eq!(report.version_rollbacks[0].active_version, "3.0.2");
    // 硬性校验通过,但安全判据强制人工确认 → 不可自动激活。
    assert!(report.passed, "版本回退不是硬性失败");
    assert!(report.needs_user_confirm, "版本回退必须强制人工确认");
    assert!(!report.auto_activatable(), "需确认时不可自动激活");
}

#[test]
fn version_upgrade_no_confirm_needed() {
    // candidate 升级 openssl 3.0.1 → 3.0.2,非回退,无需确认。
    let store = Store::open(tmp("upgrade")).unwrap();
    let idx = index(&[
        ("openssl", "3.0.1", ""),
        ("openssl", "3.0.2", ""),
    ]);
    let active = lock("clo-active2", &[("openssl", "3.0.1")]);
    let cand = lock("clo-cand2", &[("openssl", "3.0.2")]);

    let report = verify(&cand, Some(&active), &idx, &store, &[], &[], None);
    assert!(report.version_rollbacks.is_empty(), "升级不应标回退");
    assert!(!report.needs_user_confirm);
    assert!(report.auto_activatable());
}

#[test]
fn first_install_no_active_no_rollback() {
    // 首次安装(active_lock = None),不做回退比较。
    let store = Store::open(tmp("first")).unwrap();
    let idx = index(&[("openssl", "3.0.1", "")]);
    let cand = lock("clo-first", &[("openssl", "3.0.1")]);

    let report = verify(&cand, None, &idx, &store, &[], &[], None);
    assert!(report.version_rollbacks.is_empty());
    assert!(report.auto_activatable());
}

// ---- 判据3:foundation layer 约束 ----

/// 解析一个 foundation manifest(测试辅助)。
fn manifest(pkgs: &[(&str, &str, bool)]) -> FoundationManifest {
    let mut text = String::from("[meta]\nversion = \"1.0\"\n");
    for (name, ver, required) in pkgs {
        text.push_str(&format!(
            "[foundation.{name}]\nversion = \"{ver}\"\nrequired = \"{}\"\n",
            if *required { "true" } else { "false" }
        ));
    }
    FoundationManifest::parse(&text).unwrap()
}

#[test]
fn foundation_required_package_present_and_versions_match_ok() {
    // candidate 含全部 required foundation 包且版本精确匹配 → 无 foundation_violations。
    let store = Store::open(tmp("fnd-ok")).unwrap();
    let idx = index(&[("init", "1.2.0", ""), ("app", "1.0", "")]);
    let cand = lock("clo-fnd-ok", &[("init", "1.2.0"), ("app", "1.0")]);
    let fm = manifest(&[("init", "1.2.0", true)]);

    let report = verify(&cand, None, &idx, &store, &[], &[], Some(&fm));
    assert!(report.foundation_violations.is_empty(), "{:?}", report.foundation_violations);
    assert!(report.passed);
}

#[test]
fn foundation_missing_required_package_fails() {
    // required 核心包 init 不在 candidate → 缺核心组件,硬性失败。
    let store = Store::open(tmp("fnd-missing")).unwrap();
    let idx = index(&[("app", "1.0", "")]);
    let cand = lock("clo-fnd-miss", &[("app", "1.0")]);
    let fm = manifest(&[("init", "1.2.0", true)]);

    let report = verify(&cand, None, &idx, &store, &[], &[], Some(&fm));
    assert_eq!(report.foundation_violations.len(), 1);
    assert!(report.foundation_violations[0].contains("init"));
    assert!(report.foundation_violations[0].contains("缺核心组件"));
    assert!(!report.passed, "缺 required 核心包必须硬性失败");
}

#[test]
fn foundation_version_mismatch_fails() {
    // init 在场但版本与 manifest 不符(候选 1.1.0 vs 要求 1.2.0)→ 版本违规,硬性失败。
    let store = Store::open(tmp("fnd-ver")).unwrap();
    let idx = index(&[("init", "1.1.0", "")]);
    let cand = lock("clo-fnd-ver", &[("init", "1.1.0")]);
    let fm = manifest(&[("init", "1.2.0", true)]);

    let report = verify(&cand, None, &idx, &store, &[], &[], Some(&fm));
    assert_eq!(report.foundation_violations.len(), 1);
    assert!(report.foundation_violations[0].contains("1.2.0"));
    assert!(report.foundation_violations[0].contains("1.1.0"));
    assert!(!report.passed, "foundation 包版本不符必须硬性失败");
}

#[test]
fn foundation_optional_package_absent_ok() {
    // 非 required(required=false)的 foundation 包缺失 → 不违规。
    let store = Store::open(tmp("fnd-opt")).unwrap();
    let idx = index(&[("app", "1.0", "")]);
    let cand = lock("clo-fnd-opt", &[("app", "1.0")]);
    let fm = manifest(&[("extra-tool", "2.0.0", false)]);

    let report = verify(&cand, None, &idx, &store, &[], &[], Some(&fm));
    assert!(report.foundation_violations.is_empty(), "非必装包缺失不应违规");
    assert!(report.passed);
}

#[test]
fn foundation_manifest_satisfies_closure_dependency() {
    // app 依赖 libc6,libc6 不在 candidate 但由 foundation manifest 声明 → 闭合性不再误报。
    // 这是判据3 落地修正的"foundation_provided 传空导致误报"已知局限。
    let store = Store::open(tmp("fnd-closure")).unwrap();
    let idx = index(&[("app", "1.0", "libc6 (>= 2.34)")]);
    let cand = lock("clo-fnd-clo", &[("app", "1.0")]);
    // libc6 是 foundation 提供(且 required,在场校验对 candidate 不强制——它在 foundation 层)。
    // 注:此处 libc6 不要求在 candidate,故设 required=false 避免触发判据3①缺失。
    let fm = manifest(&[("libc6", "2.34", false)]);

    let report = verify(&cand, None, &idx, &store, &[], &[], Some(&fm));
    assert!(report.unclosed_deps.is_empty(), "foundation 提供的依赖不应被误报 unclosed: {:?}", report.unclosed_deps);
    assert!(report.passed);
}

