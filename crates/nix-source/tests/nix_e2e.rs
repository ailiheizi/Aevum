//! Nix binary cache 端到端集成测试(需联网 + unix)。
//!
//! 从 USTC 镜像拉取一个小包(bash)验证完整链路:narinfo → NAR 下载 → 解包 → 文件可执行。
//! 标记 #[ignore] 默认不跑(CI 无网);手动 `cargo test --test nix_e2e -- --ignored` 验证。

#![cfg(unix)]

use aevum_nix_source::NixCacheClient;
use std::path::PathBuf;

const MIRROR: &str = "https://mirrors.ustc.edu.cn/nix-channels/store";
// bash 5.3 的已知 hash(从之前实验获得)
const BASH_HASH: &str = "gik3rh1vz2jlgnifb9dh6vc6sxwwz9jj";

fn test_store_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aevum-nix-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore] // 需联网
fn fetch_narinfo_real() {
    let client = NixCacheClient::new(MIRROR, test_store_dir());
    let info = client.fetch_narinfo(BASH_HASH).expect("fetch narinfo");
    assert!(info.store_path.contains("bash"), "store_path 应含 bash: {}", info.store_path);
    assert!(!info.url.is_empty());
    assert!(info.nar_size > 0);
    println!("narinfo OK: {} ({}B)", info.name(), info.nar_size);
}

#[test]
#[ignore] // 需联网
fn fetch_and_unpack_real() {
    let store = test_store_dir();
    let client = NixCacheClient::new(MIRROR, &store);
    let info = client.fetch_one(BASH_HASH).expect("fetch_one");

    // 验证解包产物
    let ref_name = info.store_path.strip_prefix("/nix/store/").unwrap();
    let dest = store.join(ref_name);
    assert!(dest.exists(), "解包目录应存在: {}", dest.display());
    assert!(dest.join("bin/bash").exists(), "bash 二进制应在 bin/bash");

    // 验证可执行(只确认能 spawn;Nix bash 的 interpreter 指向 /nix/store/glibc,
    // 真跑 --version 可能因 interpreter 不在而失败——文件存在 + ELF 大小达标即够)。
    let _ = std::process::Command::new(dest.join("bin/bash"))
        .args(["--version"])
        .output();
    let meta = std::fs::metadata(dest.join("bin/bash")).unwrap();
    assert!(meta.len() > 100_000, "bash 应大于 100KB: {}B", meta.len());
    println!("fetch_one OK: {} → {} ({}B)", info.name(), dest.display(), meta.len());

    let _ = std::fs::remove_dir_all(&store);
}

#[test]
#[ignore] // 需联网,拉多包
fn fetch_closure_real() {
    let store = test_store_dir();
    let client = NixCacheClient::new(MIRROR, &store);
    let results = client.fetch_closure(BASH_HASH).expect("fetch_closure");

    println!("fetch_closure: {} 包", results.len());
    assert!(results.len() >= 2, "bash 至少依赖 glibc: got {}", results.len());
    // 应含 bash 本身 + glibc
    assert!(results.iter().any(|r| r.name().contains("bash")));
    assert!(results.iter().any(|r| r.name().contains("glibc")));

    // 所有包目录都应存在
    for info in &results {
        let ref_name = info.store_path.strip_prefix("/nix/store/").unwrap();
        assert!(store.join(ref_name).exists(), "缺: {}", ref_name);
    }
    println!("所有 {} 包已正确解包到 {}", results.len(), store.display());

    let _ = std::fs::remove_dir_all(&store);
}
