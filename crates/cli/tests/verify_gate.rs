//! C 主线端到端:`verify_generation` —— AI maintainer 安全闸门接上真实世代/lock/index。
//!
//! 验证 CLI 编排层把三处数据(candidate lock 版本语义 / 世代 lock.txt 的 store 对象 /
//! Debian index 的 Depends)正确喂给 `aevum_maintainer::verify`,并复现五判据中已实现的三条:
//! 完整性、闭合性、版本回退。对应 CHANGELOG 第三十一/三十二轮、runtime/01 §3。
//!
//! unix 专有:完整性判据靠 `Store::get` 重算内容哈希(`#[cfg(unix)]`),非 unix 只验对象存在。
//! 故置于 unix 以确保完整性真比对。纯本地、确定性、不触网。

#![cfg(unix)]

use std::path::Path;

use aevum_cli::{open_generations, open_store, verify_generation, Layout};
use aevum_generation::PackageRef;
use aevum_store::FileMeta;
/// 独立临时 root,避免与真实 .aevum / 其它测试互扰。
fn fresh_layout(tag: &str) -> Layout {
    let root = std::env::temp_dir().join(format!("aevum-verify-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    Layout::new(root)
}

/// 写一份最小 Debian Packages index(段落式),覆盖测试要用的包及其 Depends。
fn write_index(layout: &Layout, paragraphs: &[(&str, &str, &str)]) {
    let mut text = String::new();
    for (name, version, depends) in paragraphs {
        text.push_str(&format!("Package: {name}\n"));
        text.push_str(&format!("Version: {version}\n"));
        if !depends.is_empty() {
            text.push_str(&format!("Depends: {depends}\n"));
        }
        text.push_str(&format!("Filename: pool/{name}_{version}.deb\n"));
        text.push('\n');
    }
    let idx = layout.index_file();
    std::fs::create_dir_all(idx.parent().unwrap()).unwrap();
    std::fs::write(&idx, text).unwrap();
}

/// 写一份文本 lock(格式同 resolve_constraints:头 + `---` + `name@version#fingerprint\tfilename`)。
fn write_lock(layout: &Layout, name: &str, pkgs: &[(&str, &str)]) {
    std::fs::create_dir_all(layout.locks_dir()).unwrap();
    let mut out = String::from("closure_id: clo-test\npackage_count: 0\nunresolved: 0\nai_assist: involved=false model=none\n---\n");
    for (n, v) in pkgs {
        out.push_str(&format!("{n}@{v}#sha256:{n}-{v}\tpool/{n}_{v}.deb\n"));
    }
    std::fs::write(layout.locks_dir().join(format!("{name}.lock")), out).unwrap();
}

/// 把 pkgs 真实入库(每包一个内容寻址对象),造一个世代,其 lock.txt 即 object_ids。
/// 返回世代 id。
fn make_gen_with_objects(layout: &Layout, gen_id: u64, pkgs: &[(&str, &str)]) {
    let store = open_store(layout).unwrap();
    let meta = FileMeta { mode: 0o644, is_symlink: false };
    let mut refs = Vec::new();
    for (name, ver) in pkgs {
        // 内容含版本,确保不同包/版本得到不同 hash。
        let content = format!("body-of-{name}-{ver}");
        let dir = store.put(name, content.as_bytes(), meta).unwrap();
        let object_id = dir.file_name().unwrap().to_string_lossy().into_owned();
        refs.push(PackageRef {
            name: (*name).to_string(),
            store_dir: dir,
            object_id,
            rel_path: Some(Path::new("usr/bin").join(name)),
        });
    }
    let gens = open_generations(layout).unwrap();
    gens.make_generation(gen_id, &refs).unwrap();
}

#[test]
fn verify_clean_candidate_auto_activatable() {
    // ripgrep 依赖 libc6,二者都在 lock + 世代内 → 完整性 + 闭合性全过,无 active 无回退 → 可自动激活。
    let layout = fresh_layout("clean");
    write_index(&layout, &[
        ("ripgrep", "13.0.0", "libc6 (>= 2.34)"),
        ("libc6", "2.34", ""),
    ]);
    write_lock(&layout, "cand", &[("ripgrep", "13.0.0"), ("libc6", "2.34")]);
    make_gen_with_objects(&layout, 1, &[("ripgrep", "13.0.0"), ("libc6", "2.34")]);

    let report = verify_generation(&layout, "cand", 1, None, None).unwrap();
    assert!(report.integrity_failures.is_empty(), "{:?}", report.integrity_failures);
    assert!(report.unclosed_deps.is_empty(), "{:?}", report.unclosed_deps);
    assert!(report.version_rollbacks.is_empty());
    assert!(report.passed);
    assert!(report.auto_activatable());
}

#[test]
fn verify_detects_unclosed_dependency() {
    // ripgrep 依赖 libc6,但 lock/世代里没有 libc6 → 闭合性失败 → 不可激活。
    let layout = fresh_layout("unclosed");
    write_index(&layout, &[("ripgrep", "13.0.0", "libc6 (>= 2.34)")]);
    write_lock(&layout, "cand", &[("ripgrep", "13.0.0")]);
    make_gen_with_objects(&layout, 1, &[("ripgrep", "13.0.0")]);

    let report = verify_generation(&layout, "cand", 1, None, None).unwrap();
    assert_eq!(report.unclosed_deps.len(), 1);
    assert_eq!(report.unclosed_deps[0].package, "ripgrep");
    assert!(report.unclosed_deps[0].requirement.contains("libc6"));
    assert!(!report.passed);
}

#[test]
fn verify_detects_version_rollback_forces_confirm() {
    // active 是 openssl 3.0.2,candidate 降到 3.0.1 → 回退,强制人工确认(passed 但不可自动激活)。
    let layout = fresh_layout("rollback");
    write_index(&layout, &[
        ("openssl", "3.0.1", ""),
        ("openssl", "3.0.2", ""),
    ]);
    write_lock(&layout, "active", &[("openssl", "3.0.2")]);
    write_lock(&layout, "cand", &[("openssl", "3.0.1")]);
    make_gen_with_objects(&layout, 2, &[("openssl", "3.0.1")]);

    let report = verify_generation(&layout, "cand", 2, Some("active"), None).unwrap();
    assert_eq!(report.version_rollbacks.len(), 1);
    assert_eq!(report.version_rollbacks[0].package, "openssl");
    assert!(report.passed, "回退不是硬性失败");
    assert!(report.needs_user_confirm, "回退强制人工确认");
    assert!(!report.auto_activatable());
}

#[test]
fn verify_detects_integrity_tamper() {
    // 入库后篡改世代引用的 store 对象内容 → 完整性重算失配。
    let layout = fresh_layout("tamper");
    write_index(&layout, &[("hello", "1.0", "")]);
    write_lock(&layout, "cand", &[("hello", "1.0")]);
    make_gen_with_objects(&layout, 1, &[("hello", "1.0")]);

    // 找到 hello 的 store 对象并篡改其内容文件。
    let store = open_store(&layout).unwrap();
    let objs = store.list_objects().unwrap();
    let hello_obj = objs.iter().find(|o| o.ends_with("-hello")).expect("hello 对象应在库");
    let victim = layout.store_dir().join(hello_obj).join("hello");
    std::fs::write(&victim, b"TAMPERED").unwrap();

    let report = verify_generation(&layout, "cand", 1, None, None).unwrap();
    assert_eq!(report.integrity_failures.len(), 1, "篡改应被完整性抓到: {report:?}");
    assert!(!report.passed);
}

// ───────────────────── activate 门禁(verify 作为 set_active 前置)─────────────────────

/// 读 active 指针指向的世代 id(None = 未设)。
fn active_gen(layout: &Layout) -> Option<u64> {
    open_generations(layout).unwrap().active_generation().unwrap()
}

/// verified 审计标记是否存在。
fn has_verified_marker(layout: &Layout, gen_id: u64) -> bool {
    layout
        .generations_dir()
        .join(format!("gen-{gen_id:03}"))
        .join("verified")
        .exists()
}

#[test]
fn activate_clean_candidate_switches_active() {
    // 干净候选:门禁通过 → active 真切到该世代 + 写 verified 标记。
    let layout = fresh_layout("act-clean");
    write_index(&layout, &[
        ("ripgrep", "13.0.0", "libc6 (>= 2.34)"),
        ("libc6", "2.34", ""),
    ]);
    write_lock(&layout, "cand", &[("ripgrep", "13.0.0"), ("libc6", "2.34")]);
    make_gen_with_objects(&layout, 1, &[("ripgrep", "13.0.0"), ("libc6", "2.34")]);

    let outcome = aevum_cli::activate_verified(&layout, "cand", 1, None, None, false).unwrap();
    assert!(outcome.activated);
    assert!(outcome.blocked_reason.is_none());
    assert_eq!(active_gen(&layout), Some(1), "active 指针应切到 gen-1");
    assert!(has_verified_marker(&layout, 1), "应写 verified 审计标记");
}

#[test]
fn activate_blocked_on_hard_fail_active_untouched() {
    // 缺依赖(硬性失败):门禁拒绝,active 不动,即便给 confirm 也不行。
    let layout = fresh_layout("act-hardfail");
    write_index(&layout, &[("ripgrep", "13.0.0", "libc6 (>= 2.34)")]);
    write_lock(&layout, "cand", &[("ripgrep", "13.0.0")]);
    make_gen_with_objects(&layout, 1, &[("ripgrep", "13.0.0")]);

    // 即使 confirm=true 也不能放行硬性失败。
    let outcome = aevum_cli::activate_verified(&layout, "cand", 1, None, None, true).unwrap();
    assert!(!outcome.activated);
    assert_eq!(outcome.blocked_reason, Some(aevum_cli::ActivateBlocked::HardFail));
    assert_eq!(active_gen(&layout), None, "硬性失败 active 不应被设置");
    assert!(!has_verified_marker(&layout, 1));
}

#[test]
fn activate_rollback_needs_confirm_then_passes() {
    // 版本回退:不给 confirm 被拒(active 不动);给 confirm 后放行激活。
    let layout = fresh_layout("act-rollback");
    write_index(&layout, &[
        ("openssl", "3.0.1", ""),
        ("openssl", "3.0.2", ""),
    ]);
    write_lock(&layout, "active", &[("openssl", "3.0.2")]);
    write_lock(&layout, "cand", &[("openssl", "3.0.1")]);
    make_gen_with_objects(&layout, 2, &[("openssl", "3.0.1")]);

    // 1. 不给 confirm:被拒,active 不动。
    let blocked = aevum_cli::activate_verified(&layout, "cand", 2, Some("active"), None, false).unwrap();
    assert!(!blocked.activated);
    assert_eq!(blocked.blocked_reason, Some(aevum_cli::ActivateBlocked::NeedsConfirm));
    assert_eq!(active_gen(&layout), None, "未确认前 active 不应切");

    // 2. 给 confirm:放行激活。
    let ok = aevum_cli::activate_verified(&layout, "cand", 2, Some("active"), None, true).unwrap();
    assert!(ok.activated);
    assert_eq!(active_gen(&layout), Some(2), "确认后应激活 gen-2");
    assert!(has_verified_marker(&layout, 2));
}

