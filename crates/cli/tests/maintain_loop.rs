//! C 主线端到端总成:`aevum maintain` 主循环 —— 求解 → propose 候选世代 → verify 门禁 → 激活。
//!
//! 对应 ai/01 主循环图。验证四段串成一条可复现链路:resolve / propose_generation /
//! activate_verified 不再各自孤立,而是一条命令跑通。
//!
//! 真下载(hello 包):网络/镜像不可达则 skip(同 milestone7);解包/校验/门禁失败是真 bug。
//! unix 专有(解包/世代 symlink)。

#![cfg(unix)]

use std::path::PathBuf;

use aevum_cli::{maintain, open_generations, Layout, DEFAULT_MIRROR};
use aevum_intent::{Intent, IntentResolver, MockIntentResolver};

fn have(tool: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn maintain_hello_runs_full_loop_and_activates() {
    let root = std::env::temp_dir().join(format!("aevum-maintain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);

    // 用独立临时 root(隔离),但从 repo .aevum 复制真实 index 过来(求解依赖),
    // 既能真跑全链路、又不污染 repo 的共享世代/active 状态。
    let repo_index = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join(".aevum").join("index").join("Packages");
    if !repo_index.exists() {
        eprintln!("SKIP maintain: 无 index {repo_index:?}(先 bash scripts/prep-index.sh)");
        return;
    }
    if !have("curl") || !have("ar") || !have("tar") || !have("xz") {
        eprintln!("SKIP maintain: 缺 curl/ar/tar/xz");
        return;
    }
    let dst_index = layout.index_file();
    std::fs::create_dir_all(dst_index.parent().unwrap()).unwrap();
    std::fs::copy(&repo_index, &dst_index).unwrap();

    // 主循环:求解 hello → propose gen-50 → verify 门禁 → 激活(无 active/无 foundation,首装必过)。
    let outcome = match maintain(
        &layout,
        &["hello".to_string()],
        DEFAULT_MIRROR,
        "maintain-test",
        50,
        None,
        None,
        false, // repair
        false, // confirm
    ) {
        Ok(o) => o,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("下载失败") || msg.contains("curl") {
                eprintln!("SKIP maintain: 网络/镜像不可达: {e}");
                return;
            }
            panic!("maintain 失败(非网络,真 bug): {e}");
        }
    };

    // ① 求解出了闭包。
    assert!(outcome.resolved_packages > 0, "求解应得到 ≥1 包");
    // ② propose 造了候选世代。
    assert_eq!(outcome.candidate_gen, 50);
    assert!(outcome.store_objects > 0, "propose 应入 store 对象");
    // ③ verify 硬性通过(首装无回退,无 foundation 约束)。
    assert!(outcome.activation.report.passed, "首装应过硬性校验: {:?}", outcome.activation.report);
    // ④ 门禁激活成功,active 切到 gen-50。
    assert!(outcome.activation.activated, "首装应自动激活");
    let gens = open_generations(&layout).unwrap();
    assert_eq!(gens.active_generation().unwrap(), Some(50), "active 应切到 gen-50");
    // verified 审计标记落地。
    assert!(
        layout.generations_dir().join("gen-050").join("verified").exists(),
        "门禁激活应写 verified 标记"
    );

    eprintln!(
        "maintain 主循环达成: 求解 {} 包 → propose gen-{} ({} 对象) → verify 通过 → 激活。C 主线端到端成立。",
        outcome.resolved_packages, outcome.candidate_gen, outcome.store_objects
    );
}

/// intent 路径前段(不触网):Mock 翻译自然语言 → resolve_constraints 写 lock。
/// 验证 `aevum maintain --intent` 真正新增的逻辑——AI 翻译的约束被确定性求解成 lock,
/// AI 只在 lock 之前介入(ADR-0003/0005)。后半段(propose→门禁)由上面的端到端测试覆盖。
#[test]
fn maintain_intent_translates_then_resolves_to_lock() {
    let root = std::env::temp_dir().join(format!("aevum-maintain-intent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);

    let repo_index = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join(".aevum").join("index").join("Packages");
    if !repo_index.exists() {
        eprintln!("SKIP maintain-intent: 无 index {repo_index:?}");
        return;
    }
    let dst_index = layout.index_file();
    std::fs::create_dir_all(dst_index.parent().unwrap()).unwrap();
    std::fs::copy(&repo_index, &dst_index).unwrap();

    // 1. Mock 翻译:"我要 python" → 含 python3(MockIntentResolver 默认规则)。
    let intent = Intent::NaturalLanguage("我要 python 环境".to_string());
    let translated = MockIntentResolver::with_defaults().resolve_intent(&intent).unwrap();
    assert!(
        translated.constraints.iter().any(|c| c.name == "python3"),
        "Mock 应把 python 意图翻成 python3: {:?}", translated.constraints
    );
    assert!(translated.assist.ai_involved, "意图翻译应标记 AI 介入(审计)");

    // 2. resolve_constraints 把 AI 翻译的约束求解成确定性 lock(写 locks/intent-test.lock)。
    let lock = aevum_cli::resolve_constraints(&layout, &translated.constraints, "intent-test", Some(&translated.assist))
        .expect("resolve_constraints");
    assert!(lock.package_count > 0, "应求解出闭包");
    assert!(lock.locked.iter().any(|p| p.name == "python3"), "lock 应含 python3");

    // 3. lock 文件落地且可被主循环后半段读回(parse_lock_file 经 maintain_from_lock 调用路径间接验证)。
    let lock_path = layout.locks_dir().join("intent-test.lock");
    assert!(lock_path.exists(), "lock 文件应写入 {lock_path:?}");

    eprintln!(
        "maintain --intent 前段达成: 自然语言意图 → Mock 翻译 {} 约束 → 确定性求解 {} 包 lock。AI 只在 lock 前介入。",
        translated.constraints.len(), lock.package_count
    );
}
