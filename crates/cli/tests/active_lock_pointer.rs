//! P0-5/P0-6 回归:`list`/`remove` 必须认**当前 active 世代**对应的 lock,
//! 而不是 `locks/` 里 mtime 最新的那个。
//!
//! 修复前:回滚到旧世代后,`list` 仍读最新 lock → 谎报装了新包。
//! 这里直接测底层契约 `record_generation_lock` + `active_lock_name`:
//! 造两个世代各记一个 lock 名,active 指向谁,active_lock_name 就返回谁——
//! 包括 rollback(set_active 回旧世代)之后。
//!
//! 不触网、不解包:只建世代目录骨架 + 写指针文件 + 切 active 指针。unix 专有(世代用 symlink)。

#![cfg(unix)]

use aevum_cli::{active_lock_name, latest_lock_name, open_generations, record_generation_lock, Layout};
use aevum_generation::PackageRef;

fn tmp_layout(tag: &str) -> Layout {
    let root = std::env::temp_dir().join(format!("aevum-active-lock-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    Layout::new(&root)
}

/// 写一个 lock 文件(内容随意,只需能被 parse;这里只测指针解析,不 parse 内容)。
fn touch_lock(layout: &Layout, name: &str) {
    let dir = layout.locks_dir();
    std::fs::create_dir_all(&dir).unwrap();
    // 最小可解析 lock 头(closure_id + package_count),够 parse_lock_file 不报错即可。
    std::fs::write(
        dir.join(format!("{name}.lock")),
        format!("closure_id: clo-{name}\npackage_count: 0\n"),
    )
    .unwrap();
}

#[test]
fn active_lock_follows_active_generation_through_rollback() {
    let layout = tmp_layout("rollback");
    let gens = open_generations(&layout).unwrap();

    // 造 gen-1(lock 名 "alpha")和 gen-2(lock 名 "beta")。空包集即可——只验证指针。
    let empty: Vec<PackageRef> = Vec::new();
    gens.make_generation(1, &empty).unwrap();
    touch_lock(&layout, "alpha");
    record_generation_lock(&layout, 1, "alpha").unwrap();
    gens.set_active(1).unwrap();

    // 装 beta 前先确保它的 lock mtime 比 alpha 新(模拟"最新 lock = beta")。
    gens.make_generation(2, &empty).unwrap();
    touch_lock(&layout, "beta");
    record_generation_lock(&layout, 2, "beta").unwrap();
    gens.set_active(2).unwrap();

    // active=gen-2 → 应解析到 beta。
    assert_eq!(active_lock_name(&layout).unwrap().as_deref(), Some("beta"));
    // 且 mtime 最新的也是 beta(此时两条路径恰好一致)。
    assert_eq!(latest_lock_name(&layout).as_deref(), Some("beta"));

    // 回滚:active 指针回 gen-1。
    gens.set_active(1).unwrap();

    // 关键:active_lock_name 必须跟着回到 alpha(修复前会错误地仍返回 mtime 最新的 beta)。
    assert_eq!(
        active_lock_name(&layout).unwrap().as_deref(),
        Some("alpha"),
        "回滚后 active_lock_name 应认 active 世代(alpha),而非最新 lock(beta)"
    );
    // 反证:mtime 最新仍是 beta —— 说明老逻辑(latest_lock_name)在回滚后确实会撒谎。
    assert_eq!(latest_lock_name(&layout).as_deref(), Some("beta"));
}

#[test]
fn active_lock_none_when_no_active_generation() {
    let layout = tmp_layout("noactive");
    // 没有任何世代/active 指针 → None(调用方据此报"无活跃世代")。
    assert_eq!(active_lock_name(&layout).unwrap(), None);
}

#[test]
fn active_lock_falls_back_to_latest_for_legacy_generation() {
    // 旧世代没有 source-lock.txt 指针:回退到 mtime 最新的 lock(尽力而为,不 panic)。
    let layout = tmp_layout("legacy");
    let gens = open_generations(&layout).unwrap();
    let empty: Vec<PackageRef> = Vec::new();
    gens.make_generation(1, &empty).unwrap();
    gens.set_active(1).unwrap();
    // 故意不调用 record_generation_lock(模拟旧世代),只放一个 lock 文件。
    touch_lock(&layout, "only");
    assert_eq!(active_lock_name(&layout).unwrap().as_deref(), Some("only"));
}
