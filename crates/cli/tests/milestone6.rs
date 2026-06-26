//! 里程碑6 端到端验收:AI 增强层(意图 → 约束 → 确定性求解)。
//!
//! 验证设计文档(docs/ai/05)核心主张:
//! - AI 增强层把意图翻译成约束,喂给已实现的 solver(零改动),产出可复现 closure_id。
//! - **可复现来自确定性闭包,不来自模型**(ADR-0005):同约束走 AI vs 不走 AI,closure_id 一致。
//! - lock 记录 ai_assist(审计),但重放不依赖它。
//! - 离线降级(ADR-0005):模型不可用时 Explicit/Mock 仍可用。
//!
//! 主验证用 MockIntentResolver(确定性、不依赖网络/key,CI 可跑);
//! 真 DeepSeek 路径需 DEEPSEEK_API_KEY,缺失则 skip。
//! 前提:`bash scripts/prep-index.sh`(真实 Debian 索引)。

use std::path::PathBuf;

use aevum_cli::{resolve, resolve_intent, Layout};
use aevum_intent::{Intent, MockIntentResolver};
use aevum_solver::Constraint;

fn repo_layout() -> Layout {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    Layout::new(repo_root.join(".aevum"))
}

#[test]
fn milestone6_intent_to_closure_reproducible() {
    let layout = repo_layout();
    if !layout.index_file().exists() {
        eprintln!("SKIP milestone6: 未找到索引,请先 `bash scripts/prep-index.sh`");
        return;
    }

    // —— AI 增强路径:自然语言意图 → Mock 翻译 → 约束 → 确定性求解 ——
    let resolver = MockIntentResolver::with_defaults();
    let intent = Intent::NaturalLanguage("我要数据科学环境".to_string());
    let (lock_ai, assist) =
        resolve_intent(&layout, &resolver, &intent, "m6-ai").expect("意图求解失败");

    assert!(assist.ai_involved, "自然语言意图应标记 AI 介入");
    assert_eq!(assist.model_id, "mock");
    assert!(lock_ai.package_count > 0, "数据科学环境闭包应非空");
    eprintln!(
        "AI 路径: 意图→约束(mock)→closure_id={} ({} 包)",
        lock_ai.closure_id, lock_ai.package_count
    );

    // —— 同一批约束,不走 AI 直接求解 ——
    // Mock 把"数据科学"翻译成 python3/numpy/pandas(见 MockIntentResolver::with_defaults)。
    let same_constraints = vec!["python3".to_string(), "numpy".to_string(), "pandas".to_string()];
    let lock_plain = resolve(&layout, &same_constraints, "m6-plain").expect("直接求解失败");

    // —— 核心断言:可复现来自确定性闭包,不来自模型(ADR-0005)——
    assert_eq!(
        lock_ai.closure_id, lock_plain.closure_id,
        "同约束走 AI vs 不走 AI,closure_id 必须一致——可复现来自确定性闭包,不来自模型"
    );
    eprintln!(
        "可复现验证: AI 路径与无AI路径 closure_id 一致 = {} (不依赖模型)",
        lock_ai.closure_id
    );

    // —— lock 记录 ai_assist(审计)——
    let lock_text = std::fs::read_to_string(layout.locks_dir().join("m6-ai.lock")).unwrap();
    assert!(
        lock_text.contains("ai_assist: involved=true model=mock"),
        "lock 应记录 ai_assist 审计字段,实得:\n{lock_text}"
    );
    eprintln!("审计验证: lock 含 ai_assist(记录 AI 参与,但重放不依赖)");

    eprintln!(
        "里程碑6 达成: AI 增强层(意图→约束→确定性求解)端到端,\
         可复现不依赖模型,ai_assist 可审计 — ADR-0003/0005 兑现"
    );
}

#[test]
fn milestone6_offline_degrade() {
    // ADR-0005 离线降级:无匹配/模型不可用时 Err,但 Explicit 透传仍可用。
    let layout = repo_layout();
    if !layout.index_file().exists() {
        eprintln!("SKIP milestone6_offline: 未找到索引");
        return;
    }
    let resolver = MockIntentResolver::with_defaults();

    // 无匹配规则 → Err(调用方降级)。
    let unknown = Intent::NaturalLanguage("量子计算环境".to_string());
    assert!(
        resolve_intent(&layout, &resolver, &unknown, "m6-unknown").is_err(),
        "无匹配意图应 Err,触发降级"
    );

    // Explicit 透传 → 确定性核心仍可用(离线降级路径)。
    let explicit = Intent::Explicit(vec![Constraint::unconstrained("coreutils")]);
    let (lock, assist) =
        resolve_intent(&layout, &resolver, &explicit, "m6-explicit").expect("显式约束应成功");
    assert!(!assist.ai_involved, "显式约束不标记 AI 介入");
    assert!(lock.package_count > 0);
    eprintln!("离线降级验证: 无匹配意图 Err,Explicit 透传确定性核心仍可用(ADR-0005)");
}

/// 真 DeepSeek 端到端(需 DEEPSEEK_API_KEY + curl + 网络),缺则 skip。
#[test]
fn milestone6_deepseek_real() {
    let layout = repo_layout();
    if !layout.index_file().exists() {
        eprintln!("SKIP milestone6_deepseek: 未找到索引");
        return;
    }
    let resolver = match aevum_intent::DeepSeekResolver::from_env() {
        Some(r) => r,
        None => {
            eprintln!("SKIP milestone6_deepseek: 未设 DEEPSEEK_API_KEY(离线/CI 跳过)");
            return;
        }
    };
    let intent = Intent::NaturalLanguage("一个 Python 开发环境".to_string());
    match resolve_intent(&layout, &resolver, &intent, "m6-deepseek") {
        Ok((lock, assist)) => {
            assert!(assist.ai_involved);
            assert_eq!(assist.model_id, "deepseek-chat");
            assert!(lock.package_count > 0, "DeepSeek 翻译应解出非空闭包");
            eprintln!(
                "DeepSeek 真路径: 意图→DeepSeek→closure_id={} ({} 包)\n  约束: {}",
                lock.closure_id, lock.package_count, assist.reason
            );
        }
        Err(e) => {
            // 网络波动不算测试失败(真外部依赖),记录后跳过。
            eprintln!("SKIP milestone6_deepseek: DeepSeek 调用失败(网络/配额): {e}");
        }
    }
}
