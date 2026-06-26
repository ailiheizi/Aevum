//! 里程碑1 端到端验收:装一个 rg 并回滚(kickoff §3)。
//!
//! 直接调各 crate 库 API 串 6 步,每步断言;只第4步真跑 `rg --version`。
//! 全测试 unix 专有(symlink+rename / loader 注入 / 权限位),仅在 WSL/真 Linux 跑。
//! 前提:先 `bash scripts/prep-rg.sh` 把 rg 解到 $AEVUM_ROOT/unpacked/rg;
//! fixture 缺失则 skip(避免在没准备数据的环境假红)。

#![cfg(unix)]

use std::path::PathBuf;

use aevum_cli::{
    build, ingest_closure, open_generations, open_store, run_isolated, Layout,
};
use aevum_generation::PackageRef;
use aevum_store::{read_meta, FileMeta};

/// 用仓库内的 .aevum 作为 layout(prep-rg.sh 解包到此)。
fn repo_layout() -> Layout {
    // tests 的 CARGO_MANIFEST_DIR = crates/cli;仓库根需上溯两级。
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    Layout::new(repo_root.join(".aevum"))
}

#[test]
fn milestone1_install_and_rollback() {
    let layout = repo_layout();
    let rg_unpacked = layout.unpacked_dir("rg").join("usr/bin/rg");
    if !rg_unpacked.exists() {
        eprintln!(
            "SKIP milestone1: 未找到 {rg:?},请先 `bash scripts/prep-rg.sh`",
            rg = rg_unpacked
        );
        return;
    }

    // —— 步骤1+2:补闭包 → 解析到真实库文件 ——
    let built = build(&layout, "rg", std::path::Path::new("usr/bin/rg"))
        .expect("build_closure_resolved 失败");
    assert!(
        built.missing.is_empty(),
        "宿主应能解齐 rg 全部依赖,缺失: {:?}",
        built.missing
    );
    assert!(built.interpreter.is_some(), "rg 是动态可执行,应解出 PT_INTERP loader");
    // rg 至少依赖 libc;PoC-4 实测还有 libgcc_s/libpcre2
    let lib_names: Vec<&str> = built.libs.iter().map(|(s, _)| s.as_str()).collect();
    assert!(
        lib_names.iter().any(|s| s.starts_with("libc.so")),
        "应解出 libc,实得 {lib_names:?}"
    );

    // —— 步骤3:闭包内每个文件内容寻址入库 ——
    let store = open_store(&layout).expect("open store");
    let ingested = ingest_closure(&store, &built).expect("ingest_closure");
    assert!(
        ingested.refs.len() >= built.libs.len() + 1,
        "入库对象数应 ≥ 库数+主二进制"
    );
    // 每个入库对象都可校验取出(加载期 hash 校验通过)
    let objs = store.list_objects().expect("list_objects");
    assert!(!objs.is_empty(), "store 应有对象");

    // —— 步骤4:轻隔离运行 rg --version(PoC-2:显式 loader + --library-path)——
    let loader = ingested
        .interpreter_dir
        .as_ref()
        .expect("应有 loader 对象目录");
    let loader_name = built
        .interpreter
        .as_ref()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let loader_bin = loader.join(loader_name);
    let rg_bin = ingested.main_store_dir.join(&ingested.main_name);

    let out = run_isolated(&loader_bin, &rg_bin, &ingested.lib_dirs, &["--version"])
        .expect("run_isolated 执行失败");
    assert!(
        out.status.success(),
        "rg --version 应 rc=0,实得 {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ripgrep"),
        "rg --version 输出应含 'ripgrep',实得: {stdout}"
    );

    // —— 步骤5:造 gen-1 + 设 active,再造 gen-2(改版本)切换 + 回滚 ——
    let gens = open_generations(&layout).expect("open generations");
    gens.make_generation(1, &ingested.refs).expect("make gen-1");
    gens.set_active(1).expect("set_active 1");
    assert_eq!(gens.active_generation().unwrap(), Some(1));

    // gen-2:对 rg 内容追加无害 marker 造不同 hash(单包无第二版本,见计划)。
    // ELF 尾部追加不影响执行。共享库走同一 object_id(GC 验收前提)。
    let mut rg_v2 = std::fs::read(&rg_bin).unwrap();
    rg_v2.extend_from_slice(b"\n# aevum-gen2-marker\n");
    let v2_meta = read_meta(&rg_bin).unwrap_or(FileMeta { mode: 0o755, is_symlink: false });
    let rg_v2_dir = store.put("rg", &rg_v2, v2_meta).expect("put rg v2");
    let rg_v2_obj = rg_v2_dir.file_name().unwrap().to_string_lossy().into_owned();

    // gen-2 = 共享库(同 object_id)+ marker-rg
    let mut refs_v2: Vec<PackageRef> = ingested
        .refs
        .iter()
        .filter(|r| r.name != ingested.main_name)
        .cloned()
        .collect();
    refs_v2.push(PackageRef {
        name: ingested.main_name.clone(),
        object_id: rg_v2_obj.clone(),
        store_dir: rg_v2_dir.clone(),
        rel_path: Some(std::path::PathBuf::from("bin").join(&ingested.main_name)),
    });
    gens.make_generation(2, &refs_v2).expect("make gen-2");
    gens.set_active(2).expect("set_active 2");
    assert_eq!(gens.active_generation().unwrap(), Some(2), "切到 gen-2");

    // 回滚到 gen-1(指针回指,不重建)
    gens.rollback(1).expect("rollback 1");
    assert_eq!(gens.active_generation().unwrap(), Some(1), "回滚到 gen-1");

    // —— 步骤6:GC——只保留 gen-1,marker-rg 应回收,共享库不误删 ——
    let all = store.list_objects().expect("list_objects 2");
    let plan = gens.compute_garbage(&[1], &all).expect("compute_garbage");
    assert!(
        plan.garbage.contains(&rg_v2_obj),
        "gen-2 独占的 marker-rg 应被回收,garbage={:?}",
        plan.garbage
    );
    // 共享库(gen-1/gen-2 都引用)必须在 kept
    let libc_obj = ingested
        .refs
        .iter()
        .find(|r| r.name.starts_with("libc.so"))
        .map(|r| r.object_id.clone());
    if let Some(libc) = libc_obj {
        assert!(
            plan.kept.contains(&libc),
            "共享 libc 不能误删,kept={:?}",
            plan.kept
        );
        assert!(!plan.garbage.contains(&libc), "共享 libc 不在 garbage");
    }

    eprintln!(
        "里程碑1 达成: rg 装→跑(rc0)→切 gen-2→回滚 gen-1→GC(回收{},保留{})",
        plan.garbage.len(),
        plan.kept.len()
    );
}
