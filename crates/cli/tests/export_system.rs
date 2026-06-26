//! export_system 集成测试:验证从世代导出可运行 rootfs(nspawn/chroot 所需骨架)。
//!
//! 用合成世代(含模拟 busybox-static)验证:
//! - export_bootroot 的文件树完整铺出
//! - /etc/passwd, /etc/group, /etc/hostname 骨架补齐
//! - /bin/sh 软链指向世代内的 busybox
//! - 无 busybox 时 shell_found=false
//!
//! unix 专有(世代用 symlink)。不触网。

#![cfg(unix)]

use std::path::PathBuf;

use aevum_cli::{export_system, Layout};
use aevum_store::{FileMeta, IngestedEntry};

/// 在 store 里手造一个模拟 busybox-static 对象。
fn put_busybox(store_root: &std::path::Path) -> (PathBuf, IngestedEntry) {
    let hash = "bbbb1111";
    let obj_name = "busybox-static";
    let dir = store_root.join(format!("{hash}-{obj_name}"));
    std::fs::create_dir_all(&dir).unwrap();
    // 写一个可执行"假 busybox"(shell 脚本,能被 chroot 执行)
    let bin = dir.join(obj_name);
    std::fs::write(&bin, "#!/bin/sh\necho aevum-busybox\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let entry = IngestedEntry {
        rel_path: PathBuf::from("usr/bin/busybox-static"),
        object_id: format!("{hash}-{obj_name}"),
        store_dir: dir.clone(),
        meta: FileMeta { mode: 0o755, is_symlink: false },
    };
    (dir, entry)
}

/// 建合成世代(含 busybox-static)。
fn setup_gen_with_busybox(tag: &str) -> (Layout, u64) {
    let root = std::env::temp_dir().join(format!("aevum-expsys-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);
    let store_root = root.join("store");
    std::fs::create_dir_all(&store_root).unwrap();

    let (_dir, entry) = put_busybox(&store_root);

    // make_generation 需要 PackageRef;直接手建世代目录(与 keep_two_isolation 同手法)。
    let gens_dir = root.join("generations");
    std::fs::create_dir_all(&gens_dir).unwrap();
    let gen_dir = gens_dir.join("gen-001");
    let pkg_dir = gen_dir.join("packages");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    // 建 packages/<rel_path> → store 对象的 symlink(模拟 make_generation 行为)。
    let link = pkg_dir.join(&entry.rel_path);
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&entry.store_dir, &link).unwrap();
    // lock.txt(generation_refs 读此文件)。
    std::fs::write(gen_dir.join("lock.txt"), &entry.object_id).unwrap();

    (layout, 1)
}

#[test]
fn export_system_creates_nspawn_ready_rootfs() {
    let (layout, gen_id) = setup_gen_with_busybox("full");
    let dest = std::env::temp_dir().join(format!("aevum-rootfs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dest);

    let report = export_system(&layout, gen_id, &dest).expect("export_system");

    // /etc 骨架。
    assert!(dest.join("etc/passwd").exists(), "/etc/passwd 应存在");
    assert!(dest.join("etc/group").exists(), "/etc/group 应存在");
    assert!(dest.join("etc/hostname").exists(), "/etc/hostname 应存在");
    let passwd = std::fs::read_to_string(dest.join("etc/passwd")).unwrap();
    assert!(passwd.contains("root:x:0:0"), "passwd 应含 root");

    // /bin/sh 软链。
    assert!(report.shell_found, "应检测到 busybox-static 并建 /bin/sh");
    let sh = dest.join("bin/sh");
    assert!(sh.exists() || sh.symlink_metadata().is_ok(), "/bin/sh 应存在(软链)");
    let target = std::fs::read_link(&sh).unwrap();
    assert!(target.to_string_lossy().contains("busybox"), "/bin/sh 应指向 busybox: {target:?}");

    // 世代文件树(busybox-static 本体)。
    assert!(dest.join("usr/bin/busybox-static").exists(), "世代文件应铺出");

    // 基础目录占位。
    assert!(dest.join("proc").is_dir());
    assert!(dest.join("sys").is_dir());
    assert!(dest.join("dev").is_dir());
    assert!(dest.join("root").is_dir(), "/root home 应存在");

    // 根标志。
    assert!(dest.join("AEVUM_GENERATION_ROOT").exists());

    // 清理。
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn export_system_no_shell_reports_false() {
    // 世代不含 busybox/bash → shell_found=false(仍能导出,只是缺 shell)。
    let root = std::env::temp_dir().join(format!("aevum-expsys-nosh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let layout = Layout::new(&root);

    // 空世代(无包)。
    let gens_dir = root.join("generations");
    let gen_dir = gens_dir.join("gen-001");
    let pkg_dir = gen_dir.join("packages");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(gen_dir.join("lock.txt"), "").unwrap();

    let dest = root.join("out");
    let report = export_system(&layout, 1, &dest).expect("export_system");
    assert!(!report.shell_found, "无 busybox/bash 应报 shell_found=false");
    // /etc 骨架仍应补齐。
    assert!(dest.join("etc/passwd").exists());

    let _ = std::fs::remove_dir_all(&root);
}
