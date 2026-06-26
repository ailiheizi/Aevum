//! TS 意图前端一致性验收(ADR-0004 核心契约):
//! **同语义的 TOML/显式前端与 TS 前端,必须产出逐字节相同的 lock(closure_id 一致)。**
//!
//! 这兑现 ADR-0004 的红线:可复现只来自 lock,与用哪个前端无关。TS 是增项不是变量——
//! 它求值后被钉成与 TOML 等价的约束,走同一套确定性求解器 → 同一 closure_id。
//!
//! 需真实 Debian 索引(prep-index.sh);缺失则 skip(同 milestone5 风格)。
//! 纯文本求解,跨平台。

use std::path::PathBuf;

use aevum_cli::{resolve_constraints, Layout};
use aevum_solver::Constraint;

fn repo_layout() -> Layout {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    Layout::new(repo_root.join(".aevum"))
}

#[test]
fn toml_and_ts_frontends_produce_identical_lock() {
    let layout = repo_layout();
    if !layout.index_file().exists() {
        eprintln!("SKIP config_ts_consistency: 未找到索引,请先 `bash scripts/prep-index.sh`");
        return;
    }

    // ── 前端A:TOML/显式 —— 直接给两个顶层包名(模拟 intent.toml 的 [packages])。 ──
    let toml_constraints = vec![
        Constraint::unconstrained("coreutils"),
        Constraint::unconstrained("grep"),
    ];
    let lock_toml = resolve_constraints(&layout, &toml_constraints, "via-toml", None)
        .expect("TOML 前端求解失败");

    // ── 前端B:TS —— 同语义的 aevum.config.ts(use 同两个包,不经模板)。 ──
    // 注:useTemplate 现在表示"选用模板"(名字进 templates,交 CLI 展开),
    // 故这里用 defineSystem 返回声明式对象 + sys.use 直接带入包名,与 TOML 前端等价。
    let ts = r#"
import { defineSystem } from "@aevum/sdk";
export default defineSystem((inputs) => {
  return { uses: ["coreutils", "grep"] };
});
"#;
    let ts_constraints =
        aevum_config_ts::eval_to_constraints(ts, None).expect("TS 配置求值失败");
    let lock_ts = resolve_constraints(&layout, &ts_constraints, "via-ts", None)
        .expect("TS 前端求解失败");

    // ── 核心断言:两前端 closure_id 必须逐字节一致。 ──
    assert_eq!(
        lock_toml.closure_id, lock_ts.closure_id,
        "同语义 TOML 与 TS 前端必须产出相同 closure_id(ADR-0004:可复现只来自 lock,与前端无关)\n\
         TOML: {} ({} 包)\n  TS: {} ({} 包)",
        lock_toml.closure_id, lock_toml.package_count, lock_ts.closure_id, lock_ts.package_count
    );
    assert_eq!(lock_toml.package_count, lock_ts.package_count, "包数也应一致");

    eprintln!(
        "一致性达成: TOML 与 TS 前端同产 closure_id={} ({} 包) — ADR-0004 兑现",
        lock_ts.closure_id, lock_ts.package_count
    );
}

#[test]
fn ts_override_changes_lock_vs_unconstrained() {
    // 反向证明:TS 的 override(钉版本)确实改变求解输入 → 与无约束语义不同。
    // 不需索引:只比较 eval_to_constraints 产出的约束(纯求值,跨平台)。
    let plain = aevum_config_ts::eval_to_constraints(
        r#"export default defineSystem(() => { const s = { uses: ["python3"] }; return s; });"#,
        None,
    )
    .unwrap();
    assert_eq!(plain.len(), 1);
    assert_eq!(plain[0].name, "python3");
    assert!(plain[0].op.is_none(), "无 override 应是无约束");

    let pinned = aevum_config_ts::eval_to_constraints(
        r#"export default defineSystem(() => { const s = { uses: ["python3"], overrides: { python3: "3.11" } }; return s; });"#,
        None,
    )
    .unwrap();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].name, "python3");
    assert_eq!(pinned[0].ver.as_deref(), Some("3.11"), "override 应钉版本");
}

#[test]
fn ts_inputs_recorded_determinism() {
    // 同 TS 源 + 同显式 inputs → 同约束(确定性);不同 inputs → 不同约束(可控)。
    let ts = r#"
export default defineSystem((inputs) => {
  const sys = useTemplate("base");
  if (inputs.withDev) { sys.use("dev-rust"); }
  return sys;
});
"#;
    let a1 = aevum_config_ts::eval_to_constraints(ts, Some(r#"{"withDev":true}"#)).unwrap();
    let a2 = aevum_config_ts::eval_to_constraints(ts, Some(r#"{"withDev":true}"#)).unwrap();
    assert_eq!(
        a1.iter().map(|c| &c.name).collect::<Vec<_>>(),
        a2.iter().map(|c| &c.name).collect::<Vec<_>>(),
        "同源同输入必产同约束(确定性)"
    );
    let names1: Vec<&str> = a1.iter().map(|c| c.name.as_str()).collect();
    assert!(names1.contains(&"dev-rust"));

    let b = aevum_config_ts::eval_to_constraints(ts, Some(r#"{"withDev":false}"#)).unwrap();
    let names_b: Vec<&str> = b.iter().map(|c| c.name.as_str()).collect();
    assert!(!names_b.contains(&"dev-rust"), "不同输入应产不同约束");
}
