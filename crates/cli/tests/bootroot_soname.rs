//! 阶段4a 引擎回归:export_bootroot 必须**保留库的 soname 软链**(不跳过、不解引用)。
//!
//! 背景(CHANGELOG 第27/28轮):s6 等多库依赖包,Debian 库实体名 `libX.so.A.B.C`,
//! 二进制 NEEDED 的是 SONAME `libX.so.A`(包内本就有 `libX.so.A → libX.so.A.B.C` 软链)。
//! store 正确保留了该软链对象,但 export_bootroot 旧实现用 is_file()+canonicalize
//! 把软链**跳过/解引用** → bootroot 丢 soname 链 → loader 找不到库 → s6 引导 panic。
//! 这违反 PoC-5 铁律"符号链接保留不解引用"。本测试锁住修复:bootroot 里软链仍是软链。
//!
//! 纯本地、确定性,不依赖网络。unix 专有(symlink 语义)。

#![cfg(unix)]

use std::path::{Path, PathBuf};

use aevum_cli::{export_bootroot, open_generations, Layout};
use aevum_generation::PackageRef;

/// 在 store 目录手工铺一个对象:`<hash>-<name>/<name>`,内容由 `make` 写入(实体或软链)。
/// 返回该对象目录绝对路径(作 PackageRef.store_dir)。
fn put_object(store_root: &Path, hash: &str, name: &str, make: impl FnOnce(&Path)) -> PathBuf {
    let dir = store_root.join(format!("{hash}-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    make(&dir.join(name));
    dir
}

#[test]
fn export_bootroot_preserves_soname_symlink() {
    // 独立临时 root,避免与其它测试/真实 .aevum 互扰。
    let root = std::env::temp_dir().join(format!("aevum-bootroot-soname-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);
    let store_root = layout.store_dir();
    std::fs::create_dir_all(&store_root).unwrap();

    // 1. 实体库对象:libfoo.so.1.2.3(随便点 ELF-ish 内容,这里内容不重要,只验布局/软链)。
    let real_dir = put_object(&store_root, "aaaa1111", "libfoo.so.1.2.3", |p| {
        std::fs::write(p, b"\x7fELF-fake-real-lib").unwrap();
    });
    // 2. soname 软链对象:libfoo.so.1 → libfoo.so.1.2.3(同目录相对软链,Debian 包内形态)。
    let link_dir = put_object(&store_root, "bbbb2222", "libfoo.so.1", |p| {
        std::os::unix::fs::symlink("libfoo.so.1.2.3", p).unwrap();
    });

    // 3. 造世代:两库都按 multiarch 布局入世代(实体 + soname 软链)。
    let refs = vec![
        PackageRef {
            name: "libfoo.so.1.2.3".into(),
            store_dir: real_dir,
            object_id: "aaaa1111-libfoo.so.1.2.3".into(),
            rel_path: Some(PathBuf::from("usr/lib/x86_64-linux-gnu/libfoo.so.1.2.3")),
        },
        PackageRef {
            name: "libfoo.so.1".into(),
            store_dir: link_dir,
            object_id: "bbbb2222-libfoo.so.1".into(),
            rel_path: Some(PathBuf::from("usr/lib/x86_64-linux-gnu/libfoo.so.1")),
        },
    ];
    let gens = open_generations(&layout).expect("open generations");
    gens.make_generation(70, &refs).expect("make gen-70");

    // 4. export-bootroot,断言 soname 软链被保留(不是实体副本、不是缺失)。
    let dest = root.join("bootroot-70");
    let copied = export_bootroot(&layout, 70, &dest).expect("export-bootroot");
    assert!(copied >= 2, "应导出实体 + 软链共 2 个对象,实得 {copied}");

    let lib_dir = dest.join("usr/lib/x86_64-linux-gnu");
    let soname = lib_dir.join("libfoo.so.1");
    let real = lib_dir.join("libfoo.so.1.2.3");

    // 实体库在场。
    assert!(real.is_file(), "实体库 libfoo.so.1.2.3 应在 bootroot");
    // soname 仍是软链(关键:不被解引用成实体、不被跳过)。
    let meta = std::fs::symlink_metadata(&soname).expect("soname 应存在");
    assert!(
        meta.file_type().is_symlink(),
        "soname libfoo.so.1 必须是软链(PoC-5:保留不解引用),实际 file_type={:?}",
        meta.file_type()
    );
    // 软链目标正确指向实体(loader 据此命中)。
    let target = std::fs::read_link(&soname).expect("read soname link");
    assert_eq!(
        target,
        PathBuf::from("libfoo.so.1.2.3"),
        "soname 软链应指向同目录实体 libfoo.so.1.2.3"
    );
    // 通过软链能读到实体内容(链是活的)。
    assert!(soname.exists(), "soname 软链应可解析到实体(非断链)");

    let _ = std::fs::remove_dir_all(&root);
}
