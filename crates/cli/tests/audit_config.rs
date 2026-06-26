//! 配置漂移检测 audit_config(第五十三轮,跨平台、离线、确定性):
//! 验证用 lock 记录的 inputs 重跑源 TS 配置 → 比对 closure_id:同源同输入=未漂移;改源=漂移。
//!
//! 可复现来自 lock;本命令是旁路审计——验证 lock 仍忠实于源配置。
//! 不触网(合成 index + 临时模板)、不依赖 unix。

use aevum_cli::{audit_config, read_lock_ts_inputs, ts_config_to_constraints, resolve_constraints_opt, Layout};

fn layout(tag: &str) -> Layout {
    let root = std::env::temp_dir().join(format!("aevum-audit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let l = Layout::new(&root);
    // 合成 index:够求出非空闭包。
    let mut text = String::new();
    for (p, v) in [("python3", "3.11"), ("git", "2.40"), ("ripgrep", "13.0"), ("coreutils", "9.1")] {
        text.push_str(&format!("Package: {p}\nVersion: {v}\nFilename: pool/{p}_{v}.deb\n\n"));
    }
    let idx = l.index_file();
    std::fs::create_dir_all(idx.parent().unwrap()).unwrap();
    std::fs::write(&idx, text).unwrap();
    l
}

/// 写一份 TS 配置(无模板,纯 use)并求解产 lock(带 ts_inputs)。
fn resolve_ts(l: &Layout, ts: &str, inputs: Option<&str>, lock_name: &str) {
    let (constraints, _) = ts_config_to_constraints(l, ts, inputs).expect("ts→约束");
    resolve_constraints_opt(l, &constraints, lock_name, None, false, inputs, None).expect("求解");
}

const CONFIG_A: &str = r#"
export default defineSystem((inputs) => {
  const sys = { uses: ["python3", "git"] };
  return sys;
});
"#;

#[test]
fn no_drift_same_config_same_inputs() {
    let l = layout("nodrift");
    resolve_ts(&l, CONFIG_A, Some(r#"{"role":"dev"}"#), "app");
    // 用同源同输入审计 → 未漂移。
    let report = audit_config(&l, CONFIG_A, "app", None).expect("audit");
    assert!(!report.drifted, "同源同输入不应漂移: {report}");
    assert_eq!(report.expected_closure_id, report.actual_closure_id);
    // inputs 应来自 lock 记录(role=dev)。
    assert_eq!(report.used_inputs.as_deref(), Some(r#"{"role":"dev"}"#));
}

#[test]
fn drift_when_config_changed() {
    let l = layout("drift");
    resolve_ts(&l, CONFIG_A, None, "app");
    // 改源:多 use 一个包 → closure_id 变 → 漂移。
    let config_b = r#"
export default defineSystem((inputs) => {
  return { uses: ["python3", "git", "ripgrep"] };
});
"#;
    let report = audit_config(&l, config_b, "app", None).expect("audit");
    assert!(report.drifted, "改源应漂移: {report}");
    assert_ne!(report.expected_closure_id, report.actual_closure_id);
    assert_ne!(report.expected_pkg_count, report.actual_pkg_count);
}

#[test]
fn inputs_override_changes_result() {
    // 源按 inputs 条件带包;lock 用 A 输入,审计用 --inputs B 覆盖 → 不同结果(可控)。
    let l = layout("override");
    let conditional = r#"
export default defineSystem((inputs) => {
  const u = ["python3"];
  if (inputs.withGit) { u.push("git"); }
  return { uses: u };
});
"#;
    resolve_ts(&l, conditional, Some(r#"{"withGit":true}"#), "app");
    // 用记录值(withGit:true)审计 → 未漂移。
    let same = audit_config(&l, conditional, "app", None).expect("audit");
    assert!(!same.drifted, "记录输入重放应一致: {same}");
    // override 成 withGit:false → 少一个包 → 与 lock 不同(报告为漂移,符合"换输入会怎样")。
    let diff = audit_config(&l, conditional, "app", Some(r#"{"withGit":false}"#)).expect("audit");
    assert!(diff.drifted, "覆盖输入应改变结果: {diff}");
    assert_eq!(diff.used_inputs.as_deref(), Some(r#"{"withGit":false}"#));
}

#[test]
fn read_lock_ts_inputs_roundtrip() {
    let l = layout("tsin");
    resolve_ts(&l, CONFIG_A, Some(r#"{"k":"v"}"#), "withinp");
    resolve_ts(&l, CONFIG_A, None, "noinp");
    assert_eq!(
        read_lock_ts_inputs(&l.locks_dir().join("withinp.lock")).as_deref(),
        Some(r#"{"k":"v"}"#)
    );
    assert_eq!(read_lock_ts_inputs(&l.locks_dir().join("noinp.lock")), None, "none 占位应读回 None");
}

#[test]
fn audit_missing_lock_errors() {
    let l = layout("missing");
    assert!(audit_config(&l, CONFIG_A, "nonexistent", None).is_err(), "缺 lock 应报错");
}
