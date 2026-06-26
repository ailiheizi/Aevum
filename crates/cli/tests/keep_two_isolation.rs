//! repair 方案C 运行时视图隔离端到端(ai/02 §3.2):证明"保留两份"机制成立——
//! 两个冲突 app 各建私有依赖视图,同名依赖 `libfoo.so.1` 在各自视图里指向**不同版本**的 store 对象。
//!
//! 这是 Aevum 兜底卖点"实在不行保留两份,各跑各的"的最小可验证落地:
//! 同一台机器、同一逻辑库名,两 app 各见各的版本,互不可见。
//!
//! unix 专有(symlink 视图);纯本地、确定性、不触网。

#![cfg(unix)]

use std::path::{Path, PathBuf};

use aevum_cli::materialize_isolated_views;
use aevum_store::{FileMeta, IngestedEntry};
/// 在 store 目录手工铺一个库对象:`<hash>-libfoo.so.1/libfoo.so.1`,内容由 body 决定(模拟不同版本)。
/// 返回该对象目录绝对路径。
fn put_lib(store_root: &Path, hash: &str, body: &[u8]) -> PathBuf {
    let dir = store_root.join(format!("{hash}-libfoo.so.1"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("libfoo.so.1"), body).unwrap();
    dir
}

/// 构造一个 app 对该库的依赖条目(rel_path 用 multiarch 布局,指向给定 store 对象)。
fn entry_for(store_dir: &Path, hash: &str) -> IngestedEntry {
    IngestedEntry {
        rel_path: PathBuf::from("libfoo.so.1"),
        object_id: format!("{hash}-libfoo.so.1"),
        store_dir: store_dir.to_path_buf(),
        meta: FileMeta { mode: 0o755, is_symlink: false },
    }
}

#[test]
fn isolated_views_point_to_different_versions() {
    let root = std::env::temp_dir().join(format!("aevum-keeptwo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store_root = root.join("store");
    std::fs::create_dir_all(&store_root).unwrap();

    // 两个版本的 libfoo.so.1:内容不同 → 不同 hash 对象,store 天然并存(内容寻址)。
    let v_old = put_lib(&store_root, "aaaa1111", b"libfoo-3.0.13-body");
    let v_new = put_lib(&store_root, "bbbb2222", b"libfoo-3.2.1-body");

    // app-old 要旧版、app-new 要新版(方案A/B 无解时的"保留两份")。
    let views = vec![
        ("app-old".to_string(), vec![entry_for(&v_old, "aaaa1111")]),
        ("app-new".to_string(), vec![entry_for(&v_new, "bbbb2222")]),
    ];
    let base = root.join("views");
    let dirs = materialize_isolated_views(&views, &base).expect("materialize_isolated_views");
    assert_eq!(dirs.len(), 2, "应为两个 app 各建一个视图");

    // 各视图里 libfoo.so.1 是 symlink,且 target 指向各自版本的 store 对象(关键:不同 hash)。
    let old_link = base.join("app-old").join("libfoo.so.1");
    let new_link = base.join("app-new").join("libfoo.so.1");
    let old_target = std::fs::read_link(&old_link).expect("app-old 视图应有 libfoo symlink");
    let new_target = std::fs::read_link(&new_link).expect("app-new 视图应有 libfoo symlink");

    assert!(old_target.to_string_lossy().contains("aaaa1111"), "app-old 应指向旧版对象: {old_target:?}");
    assert!(new_target.to_string_lossy().contains("bbbb2222"), "app-new 应指向新版对象: {new_target:?}");
    assert_ne!(old_target, new_target, "两 app 的同名依赖必须指向不同版本对象(隔离成立)");

    // 内容验证:各 app 通过自己的视图读到的就是各自那版库内容。
    let old_content = std::fs::read(&old_link).unwrap();
    let new_content = std::fs::read(&new_link).unwrap();
    assert_eq!(old_content, b"libfoo-3.0.13-body", "app-old 应读到旧版内容");
    assert_eq!(new_content, b"libfoo-3.2.1-body", "app-new 应读到新版内容");
}

#[test]
fn isolated_view_rejects_path_escape() {
    let root = std::env::temp_dir().join(format!("aevum-keeptwo-escape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store_root = root.join("store");
    std::fs::create_dir_all(&store_root).unwrap();
    let v = put_lib(&store_root, "cccc3333", b"x");

    // app 视图名含 `..` → 必须被拒(防视图逃逸)。
    let views = vec![("../evil".to_string(), vec![entry_for(&v, "cccc3333")])];
    let err = materialize_isolated_views(&views, root.join("views"));
    assert!(err.is_err(), "含 .. 的视图名应被拒绝");
}

#[test]
fn keep_two_views_attached_to_generation() {
    // 世代级集成(旁路):造世代主体后,把两 app 的私有视图挂进 gen-NNN/private-views/,
    // 验证私有视图各指向不同版本、keep-two.txt 记录两 app,且世代主体 packages/ 不受影响。
    use aevum_cli::{attach_keep_two_views, open_generations, Layout};
    use aevum_generation::PackageRef;

    let root = std::env::temp_dir().join(format!("aevum-keeptwo-gen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);
    let store_root = layout.store_dir();
    std::fs::create_dir_all(&store_root).unwrap();

    let v_old = put_lib(&store_root, "aaaa1111", b"libfoo-old");
    let v_new = put_lib(&store_root, "bbbb2222", b"libfoo-new");

    // 造一个最小世代主体(共享布局照旧,放一个无关主对象即可)。
    let gens = open_generations(&layout).unwrap();
    let main_obj = put_lib(&store_root, "dddd4444", b"shared-main");
    gens.make_generation(80, &[PackageRef {
        name: "libfoo.so.1".into(),
        store_dir: main_obj.clone(),
        object_id: "dddd4444-libfoo.so.1".into(),
        rel_path: Some(PathBuf::from("usr/lib/libfoo.so.1")),
    }]).unwrap();

    // 挂两 app 私有视图(冲突库各用各版本)。
    let views = vec![
        ("app-old".to_string(), vec![entry_for(&v_old, "aaaa1111")]),
        ("app-new".to_string(), vec![entry_for(&v_new, "bbbb2222")]),
    ];
    let dirs = attach_keep_two_views(&layout, 80, &views).expect("attach_keep_two_views");
    assert_eq!(dirs.len(), 2);

    // 私有视图在 gen-080/private-views/<app>/ 下,各指向不同版本对象。
    let gen_dir = gens.generation_dir(80);
    let old_link = gen_dir.join("private-views/app-old/libfoo.so.1");
    let new_link = gen_dir.join("private-views/app-new/libfoo.so.1");
    assert_eq!(std::fs::read(&old_link).unwrap(), b"libfoo-old", "app-old 私有视图应读到旧版");
    assert_eq!(std::fs::read(&new_link).unwrap(), b"libfoo-new", "app-new 私有视图应读到新版");
    assert_ne!(
        std::fs::read_link(&old_link).unwrap(),
        std::fs::read_link(&new_link).unwrap(),
        "两 app 私有视图必须指向不同版本对象"
    );

    // keep-two.txt 记录两 app。
    let manifest = std::fs::read_to_string(gen_dir.join("keep-two.txt")).unwrap();
    assert!(manifest.contains("app-old") && manifest.contains("app-new"), "keep-two.txt 应记两 app: {manifest:?}");

    // 世代主体 packages/ 不受私有视图影响(仍是共享布局的那一份)。
    assert!(gen_dir.join("packages/usr/lib/libfoo.so.1").exists(), "世代主体共享布局应照旧");

    // GC 可达性:私有视图引用的对象必须纳入可达集,不被 GC 误回收(第四十七轮修)。
    let reachable = gens.generation_object_ids(80).unwrap();
    assert!(reachable.contains(&"aaaa1111-libfoo.so.1".to_string()), "app-old 私有对象应可达: {reachable:?}");
    assert!(reachable.contains(&"bbbb2222-libfoo.so.1".to_string()), "app-new 私有对象应可达: {reachable:?}");
    // compute_garbage:把所有对象喂进去,私有视图对象应在 kept、不在 garbage。
    let all = vec![
        "aaaa1111-libfoo.so.1".to_string(),
        "bbbb2222-libfoo.so.1".to_string(),
        "dddd4444-libfoo.so.1".to_string(),
        "eeee5555-orphan".to_string(), // 无引用的孤儿,应被回收
    ];
    let plan = gens.compute_garbage(&[80], &all).unwrap();
    assert!(plan.kept.contains(&"aaaa1111-libfoo.so.1".to_string()), "私有对象不应被回收");
    assert!(plan.kept.contains(&"bbbb2222-libfoo.so.1".to_string()), "私有对象不应被回收");
    assert!(plan.garbage.contains(&"eeee5555-orphan".to_string()), "孤儿对象应被回收");
    assert!(!plan.garbage.contains(&"aaaa1111-libfoo.so.1".to_string()), "私有对象绝不能进 garbage");

    // 运行期消费(第四十八轮):private_view_dir 取该 app 私有视图作 --library-path 用。
    use aevum_cli::private_view_dir;
    let old_view = private_view_dir(&layout, 80, "app-old").expect("app-old 应有私有视图");
    let new_view = private_view_dir(&layout, 80, "app-new").expect("app-new 应有私有视图");
    // 视图目录里含该 app 那版库 symlink(ld 按 soname 命中),内容是各自版本。
    assert_eq!(std::fs::read(old_view.join("libfoo.so.1")).unwrap(), b"libfoo-old");
    assert_eq!(std::fs::read(new_view.join("libfoo.so.1")).unwrap(), b"libfoo-new");
    // 无私有视图的 app / 不存在的世代 → None(调用方回退普通运行)。
    assert!(private_view_dir(&layout, 80, "no-such-app").is_none());
    assert!(private_view_dir(&layout, 999, "app-old").is_none());
    // 视图名逃逸防护。
    assert!(private_view_dir(&layout, 80, "../evil").is_none());
}

