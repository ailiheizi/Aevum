//! repair 方案A 端到端(CLI 层,跨平台、离线、确定性):
//! 验证 `resolve_constraints_opt(repair=true)` 把互斥约束的冲突自动修到可用 lock,
//! 并在 lock 文件留下 `# applied-repair:` 审计;repair=false 时则留 `# conflict:` 诊断。
//!
//! 不触网(自带合成 index)、不依赖 unix(只读 index/写 lock 文本)。补第四十轮标注的端到端缺口。

use aevum_cli::{resolve_constraints, resolve_constraints_opt, Layout};
use aevum_solver::version::VerOp;
use aevum_solver::Constraint;

/// 临时 layout + 写一份含 libc6 三版本(2.34/2.35/2.36)的合成 index。
fn layout_with_index(tag: &str) -> Layout {
    let root = std::env::temp_dir().join(format!("aevum-repair-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);
    let mut text = String::new();
    for v in ["2.34", "2.35", "2.36"] {
        text.push_str(&format!("Package: libc6\nVersion: {v}\nFilename: pool/libc6_{v}.deb\n\n"));
    }
    let idx = layout.index_file();
    std::fs::create_dir_all(idx.parent().unwrap()).unwrap();
    std::fs::write(&idx, text).unwrap();
    layout
}

/// 互斥但有交集的约束:libc6 (>=2.35) 与 libc6 (<=2.35) → 交集恰为 2.35。
fn conflicting_constraints() -> Vec<Constraint> {
    vec![
        Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.35".into()) },
        Constraint { name: "libc6".into(), op: Some(VerOp::Le), ver: Some("2.35".into()) },
    ]
}

#[test]
fn repair_true_resolves_conflict_to_usable_lock() {
    let layout = layout_with_index("on");
    let lock = resolve_constraints_opt(&layout, &conflicting_constraints(), "rep", None, true, None, None)
        .expect("resolve_constraints_opt");

    // 自动 repair 后无冲突,libc6 钉到共存版本 2.35。
    assert!(lock.diagnostics.conflicts.is_empty(), "repair 后不应有冲突: {:?}", lock.diagnostics.conflicts);
    assert_eq!(lock.locked.iter().find(|p| p.name == "libc6").unwrap().version, "2.35");

    // lock 文件应记审计:applied-repair 出现,conflicts 为 0。
    let body = std::fs::read_to_string(layout.locks_dir().join("rep.lock")).unwrap();
    assert!(body.contains("# applied-repair: libc6 钉到 2.35"), "lock 应记 applied-repair:\n{body}");
    assert!(body.contains("conflicts: 0"), "repair 后 conflicts 应为 0:\n{body}");
}

#[test]
fn repair_false_leaves_conflict_diagnostic() {
    let layout = layout_with_index("off");
    let lock = resolve_constraints(&layout, &conflicting_constraints(), "norep", None)
        .expect("resolve_constraints");

    // 不 repair:冲突被检测并留诊断(不自动放宽)。
    assert!(!lock.diagnostics.conflicts.is_empty(), "未 repair 应保留冲突诊断");
    let body = std::fs::read_to_string(layout.locks_dir().join("norep.lock")).unwrap();
    assert!(body.contains("# conflict:"), "lock 应记 conflict 诊断:\n{body}");
    // 方案A 建议仍在(可放宽到 2.35),但未自动应用。
    assert!(body.contains("# repair-A: libc6 → 可放宽到 2.35"), "应有方案A 建议:\n{body}");
    assert!(!body.contains("# applied-repair"), "未 repair 不应有 applied-repair:\n{body}");
}

/// 写一份触发方案B 的合成 index:openssl 仅 3.0/3.2(A 对 <<3.1 与 >=3.2 无解);
/// app-x 有 1.0(依赖 openssl<<3.1)与 1.2(放宽 openssl>=3.0);app-y 依赖 openssl>=3.2。
fn layout_with_index_b(tag: &str) -> Layout {
    let root = std::env::temp_dir().join(format!("aevum-repair-b-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);
    let text = "\
Package: openssl
Version: 3.0
Filename: pool/openssl_3.0.deb

Package: openssl
Version: 3.2
Filename: pool/openssl_3.2.deb

Package: app-x
Version: 1.0
Depends: openssl (<< 3.1)
Filename: pool/app-x_1.0.deb

Package: app-x
Version: 1.2
Depends: openssl (>= 3.0)
Filename: pool/app-x_1.2.deb

Package: app-y
Version: 1.0
Depends: openssl (>= 3.2)
Filename: pool/app-y_1.0.deb
";
    let idx = layout.index_file();
    std::fs::create_dir_all(idx.parent().unwrap()).unwrap();
    std::fs::write(&idx, text).unwrap();
    layout
}

#[test]
fn repair_b_suggestion_written_to_lock() {
    // app-x 钉 <<1.1(选 1.0,依赖 openssl<<3.1)+ app-y(openssl>=3.2)→ openssl 无共存版本(A 失败)。
    // 方案B:升级 app-x 到 1.2 → openssl 取 3.2。lock 应记 # repair-B。
    let layout = layout_with_index_b("lock");
    let constraints = vec![
        Constraint { name: "app-x".into(), op: Some(VerOp::Lt), ver: Some("1.1".into()) },
        Constraint::unconstrained("app-y"),
    ];
    let lock = resolve_constraints(&layout, &constraints, "repb", None).expect("resolve");

    // openssl 冲突、方案A 无解。
    assert!(lock.diagnostics.conflicts.iter().any(|c| c.package == "openssl"));
    let body = std::fs::read_to_string(layout.locks_dir().join("repb.lock")).unwrap();
    assert!(body.contains("# repair-A: openssl → 无单一共存版本"), "应记方案A 无解:\n{body}");
    assert!(body.contains("# repair-B: 升级 app-x 到 1.2 → openssl 取 3.2"), "应记方案B 建议:\n{body}");
}

#[test]
fn repair_c_keeps_two_to_lock() {
    // 顶层互斥 libc6 (>=2.36) 与 (<=2.34):A 无单一共存版本、B 无父包可升,
    // 但两方各自可满足(2.36 / 2.34)→ 方案C 保留两份:lock 记 # repair-C(需确认)。
    let layout = layout_with_index("c");
    let constraints = vec![
        Constraint { name: "libc6".into(), op: Some(VerOp::Ge), ver: Some("2.36".into()) },
        Constraint { name: "libc6".into(), op: Some(VerOp::Le), ver: Some("2.34".into()) },
    ];
    let lock = resolve_constraints(&layout, &constraints, "repc", None).expect("resolve");
    assert!(!lock.diagnostics.conflicts.is_empty(), "应有冲突");
    assert!(lock.diagnostics.keep_two_suggestions.iter().any(|c| c.package == "libc6"), "应建议保留两份");
    assert!(lock.diagnostics.unrepairable.is_empty(), "C 能兜底则不落 D");
    let body = std::fs::read_to_string(layout.locks_dir().join("repc.lock")).unwrap();
    assert!(body.contains("# repair-C: libc6 保留两份 2.34 与 2.36"), "lock 应记方案C:\n{body}");
}


