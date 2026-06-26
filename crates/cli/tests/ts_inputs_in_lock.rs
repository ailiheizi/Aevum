//! TS inputs 记入 lock(ADR-0004 持久化收尾,跨平台、离线、确定性):
//! 验证 `resolve_constraints_opt` 的 `inputs` 参数把 TS 前端的显式输入写进 lock 头部审计区
//! (`ts_inputs:` 行,在 `---` 之前),且**不影响 closure_id**(inputs 是审计元数据,
//! 不进 build_lock 的内容摘要 → 可复现只来自 resolved 包集,与上游 inputs 无关)。
//!
//! 这补全 ADR-0004"输入被记录进 lock"的最小增量(只写不验、只记不用,见 CHANGELOG 边界)。
//! 不触网(合成 index)、不依赖 unix(只读 index/写读 lock 文本)。

use aevum_cli::{parse_lock_file, resolve_constraints_opt, Layout};
use aevum_solver::Constraint;

/// 临时 layout + 写一份含 libc6 单版本的合成 index(够求出非空闭包即可)。
fn layout_with_index(tag: &str) -> Layout {
    let root = std::env::temp_dir().join(format!("aevum-ts-inputs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);
    let text = "Package: libc6\nVersion: 2.36\nFilename: pool/libc6_2.36.deb\n\n";
    let idx = layout.index_file();
    std::fs::create_dir_all(idx.parent().unwrap()).unwrap();
    std::fs::write(&idx, text).unwrap();
    layout
}

fn one_pkg() -> Vec<Constraint> {
    vec![Constraint::unconstrained("libc6")]
}

#[test]
fn inputs_written_to_lock_single_lined() {
    let layout = layout_with_index("write");
    // inputs 故意含 \n \t:应被单行化为空格(防破坏按行解析)。
    let lock = resolve_constraints_opt(
        &layout,
        &one_pkg(),
        "with-inputs",
        None,
        false,
        Some("role=developer\ttools=[rg]\nenv=prod"),
        None,
    )
    .expect("resolve");
    let _ = lock;

    let path = layout.locks_dir().join("with-inputs.lock");
    let text = std::fs::read_to_string(&path).expect("读 lock");

    // ts_inputs 行存在、单行化(\n\t → 空格)、出现在 `---` 之前(头部审计区)。
    let (head, _body) = text.split_once("\n---\n").expect("lock 应有 --- 分隔");
    assert!(
        head.contains("ts_inputs: role=developer tools=[rg] env=prod"),
        "ts_inputs 应单行化写进头部: {head:?}"
    );
    assert!(!head.contains("ts_inputs: role=developer\t"), "制表符应被替换");
}

#[test]
fn no_inputs_writes_none_placeholder() {
    let layout = layout_with_index("none");
    resolve_constraints_opt(&layout, &one_pkg(), "no-inputs", None, false, None, None).expect("resolve");
    let text = std::fs::read_to_string(layout.locks_dir().join("no-inputs.lock")).unwrap();
    assert!(text.contains("ts_inputs: none"), "无 inputs 应写占位行: {text}");
}

#[test]
fn inputs_do_not_affect_closure_id() {
    // 核心:同约束、不同 inputs(甚至 None)→ 必产相同 closure_id。
    // 证明 inputs 是审计元数据,不进 build_lock 的内容摘要(确定性不被破坏)。
    let layout = layout_with_index("det");
    let a = resolve_constraints_opt(&layout, &one_pkg(), "a", None, false, Some("x=1"), None).expect("a");
    let b = resolve_constraints_opt(&layout, &one_pkg(), "b", None, false, Some("y=2"), None).expect("b");
    let c = resolve_constraints_opt(&layout, &one_pkg(), "c", None, false, None, None).expect("c");
    assert_eq!(a.closure_id, b.closure_id, "不同 inputs 不应改变 closure_id");
    assert_eq!(a.closure_id, c.closure_id, "有无 inputs 不应改变 closure_id");
    assert!(!a.closure_id.is_empty() && a.closure_id.starts_with("clo-"));
}

#[test]
fn lock_with_inputs_parses_back_clean() {
    // 回读:带 ts_inputs 段的 lock 仍能被 parse_lock_file 正常解析,
    // 且 ts_inputs 内容不混进包体(头部行被宽松忽略)。
    let layout = layout_with_index("parse");
    let written = resolve_constraints_opt(
        &layout,
        &one_pkg(),
        "rt",
        None,
        false,
        Some("role=dev"),
        None,
    )
    .expect("resolve");
    let path = layout.locks_dir().join("rt.lock");
    let parsed = parse_lock_file(&path).expect("parse_lock_file 应成功");
    assert_eq!(parsed.closure_id, written.closure_id, "回读 closure_id 一致");
    assert_eq!(parsed.package_count, written.package_count, "回读包数一致");
    assert!(
        parsed.locked.iter().all(|p| !p.name.contains("ts_inputs") && !p.name.contains("role")),
        "ts_inputs 内容不应混进包体"
    );
}
