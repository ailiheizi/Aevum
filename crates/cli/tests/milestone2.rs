//! 里程碑2 端到端验收:复杂包(python)——补上 PoC-5 的 dlopen 盲区。
//!
//! PoC-5 发现:python 主二进制 NEEDED 只有 libpython/libc,但 `import ssl` 运行时
//! dlopen `lib-dynload/_ssl*.so` → 再 NEEDED libssl/libcrypto,整条链 ELF 静态分析看不见。
//! 本测试证明 Aevum 的"扫全包 ELF + 递归"把这条链补回来了。
//!
//! 验证 A(补闭包完整性):scanned_elf ≥ 78、resolved 含 libssl/libcrypto、missing 收敛。
//! 验证 B(真跑 import ssl):轻隔离 python3 -c "import ssl" rc=0 = dlopen 闭包闭合。
//!
//! unix 专有;fixture(prep-complex.sh 解包 python)缺失则 skip。

#![cfg(unix)]

use std::path::{Path, PathBuf};

use aevum_cli::{build_with, ingest_closure, open_store, Layout};

fn repo_layout() -> Layout {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    Layout::new(repo_root.join(".aevum"))
}

/// 真实主程序是 usr/bin/python3.14(python3 是它的软链)。
const PY_BIN: &str = "usr/bin/python3.14";
const PY_RUNTIME: &str = "usr/lib/python3.14";

#[test]
fn milestone2_python_dlopen_closure() {
    let layout = repo_layout();
    let py = layout.unpacked_dir("python").join(PY_BIN);
    if !py.exists() {
        eprintln!("SKIP milestone2: 未找到 {py:?},请先 `bash scripts/prep-complex.sh py`");
        return;
    }

    // —— 验证 A:补闭包完整性(PoC-5 核心)——
    let built = build_with(&layout, "python", Path::new(PY_BIN), &[PathBuf::from(PY_RUNTIME)])
        .expect("build_with python 失败");

    // 必须扫了全包 ELF,而非只主二进制(77 lib-dynload + 主 + libpython ≥ 78)。
    assert!(
        built.scanned_elf_count >= 78,
        "应扫全包 ELF(≥78),实得 {}——退化为只扫主二进制则复杂包必崩(PoC-5)",
        built.scanned_elf_count
    );

    // 决定性断言:libssl/libcrypto 是 _ssl.so 的传递依赖,主二进制 NEEDED 看不见。
    // 扫全包 ELF + 递归才能把它们抓回来——这正是 PoC-4 的盲区。
    let lib_names: Vec<&str> = built.libs.iter().map(|(s, _)| s.as_str()).collect();
    let has_ssl = lib_names.iter().any(|s| s.starts_with("libssl.so"));
    let has_crypto = lib_names.iter().any(|s| s.starts_with("libcrypto.so"));
    assert!(
        has_ssl && has_crypto,
        "应从 _ssl.so 递归解出 libssl + libcrypto(PoC-4 盲区被扫全包补回),实得 {lib_names:?}"
    );

    // libpython 必须从包内解出(宿主没有)。
    assert!(
        lib_names.iter().any(|s| s.starts_with("libpython3")),
        "应从包内解出 libpython3.14,实得 {lib_names:?}"
    );

    eprintln!(
        "验证A 通过: 扫 {} ELF, 解出 {} 库(含 libssl/libcrypto), 缺失 {} 个: {:?}",
        built.scanned_elf_count,
        built.libs.len(),
        built.missing.len(),
        built.missing
    );

    // —— 入库(含运行时目录整体纳入,源3/4)——
    let store = open_store(&layout).expect("open store");
    let ingested = ingest_closure(&store, &built).expect("ingest_closure");
    // 运行时目录(标准库 + lib-dynload)应有大量对象入库
    assert!(
        ingested.runtime_objs.len() > 100,
        "python 标准库整目录应入库(100+ 对象),实得 {}",
        ingested.runtime_objs.len()
    );
    eprintln!("入库运行时对象 {} 个", ingested.runtime_objs.len());

    // —— 验证 B:轻隔离真跑 import ssl(证 dlopen 闭包闭合)——
    // 用解包目录作运行视图(布局完整),库走宿主/包内解出的真实路径;
    // 关键证明:import ssl 不崩 = _ssl.so 的依赖链(libssl/libcrypto)在闭包里。
    let loader_dir = ingested.interpreter_dir.as_ref().expect("应有 loader");
    let loader_name = built
        .interpreter
        .as_ref()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let loader = loader_dir.join(loader_name);

    let pkg_root = layout.unpacked_dir("python");
    // 真实 python3.14 二进制(用解包目录,保 PYTHONHOME 布局)。
    let py_bin = pkg_root.join(PY_BIN);

    // library-path:解出的库目录(含 libpython/libssl/libcrypto)。
    let py_home = pkg_root.join("usr");
    let out = run_isolated_with_env(
        &loader,
        &py_bin,
        &ingested.lib_dirs,
        &["-c", "import ssl; print(ssl.OPENSSL_VERSION)"],
        &[("PYTHONHOME", py_home.to_str().unwrap())],
    )
    .expect("run python 失败");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "python import ssl 应 rc=0(dlopen 闭包闭合),实得 {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("OpenSSL"),
        "应打印 OpenSSL 版本,实得 stdout: {stdout}\nstderr: {stderr}"
    );

    eprintln!("验证B 通过: import ssl rc=0, {}", stdout.trim());
    eprintln!("里程碑2 达成: 复杂包 python 补全 dlopen 闭包(PoC-5 盲区已补),import ssl 不崩");
}

/// run_isolated 的带环境变量版本(python 需 PYTHONHOME 找标准库)。
fn run_isolated_with_env(
    loader: &Path,
    bin: &Path,
    lib_dirs: &[PathBuf],
    args: &[&str],
    env: &[(&str, &str)],
) -> std::io::Result<std::process::Output> {
    let lib_path = lib_dirs
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");
    let mut cmd = std::process::Command::new(loader);
    cmd.arg("--library-path").arg(&lib_path).arg(bin).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output()
}
