//! 里程碑5 端到端验收:solver 接真实 Debian 索引,确定性求解。
//!
//! solver 此前从未用真实数据端到端验证(单测都手构造小索引)。本测试用真实 Debian
//! Packages(prep-index.sh 解压的 ~6.8 万包)兑现 ADR-0003:
//! **AI 产意图(顶层包名),确定性求解器算闭包并产 lock,可复现只来自 lock。**
//!
//! 验证(PoC-3 铁律):真实索引解析无崩、闭包传递正确、closure_id 两次一致(可复现)。
//! 跨平台(纯文本);fixture(prep-index.sh)缺失则 skip。

use std::path::PathBuf;

use aevum_cli::{resolve, Layout};

fn repo_layout() -> Layout {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    Layout::new(repo_root.join(".aevum"))
}

#[test]
fn milestone5_resolve_real_debian_index() {
    let layout = repo_layout();
    if !layout.index_file().exists() {
        eprintln!("SKIP milestone5: 未找到索引,请先 `bash scripts/prep-index.sh`");
        return;
    }

    // 用 coreutils(依赖明确的真实包)求解。
    let pkgs = vec!["coreutils".to_string()];
    let lock1 = resolve(&layout, &pkgs, "coreutils").expect("resolve coreutils 失败");

    // 真实索引应解出非空闭包,且 closure_id 有格式。
    assert!(lock1.package_count > 0, "coreutils 闭包应非空");
    assert!(lock1.closure_id.starts_with("clo-"), "closure_id 格式: {}", lock1.closure_id);
    // coreutils 必依赖 libc6(传递闭包正确性)。
    assert!(
        lock1.locked.iter().any(|p| p.name == "libc6"),
        "coreutils 闭包应含 libc6,实得 {:?}",
        lock1.locked.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    eprintln!(
        "解析真实索引: coreutils 闭包 {} 包, closure_id={}, 未解析 {}",
        lock1.package_count,
        lock1.closure_id,
        lock1.diagnostics.unresolved.len()
    );

    // PoC-3 可复现铁律:同输入两次求解,closure_id 必须一致。
    let lock2 = resolve(&layout, &pkgs, "coreutils-2").expect("二次 resolve 失败");
    assert_eq!(
        lock1.closure_id, lock2.closure_id,
        "同输入两次求解 closure_id 必须一致(确定性可复现,无随机/时钟/AI)"
    );
    assert_eq!(lock1.package_count, lock2.package_count);
    eprintln!("可复现验证: 两次求解 closure_id 一致 = {}", lock1.closure_id);

    eprintln!(
        "里程碑5 达成: solver 接真实 Debian 索引(~6.8万包),coreutils 确定性求解 + lock 产出,\
         closure_id 可复现 — ADR-0003(AI 产意图、确定性求解器算闭包)兑现"
    );
}
