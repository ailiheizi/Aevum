//! 里程碑4 端到端验收:外部依赖纳入闭包 + im 真转图 + 同源硬约束。
//!
//! 块1+2:装齐 delegate(scripts/prep-im-delegates.sh)后,im 闭包解出核心 delegate
//!        (liblcms2/libjpeg/libpng…,来自宿主 = cross_source),轻隔离真转一张 PNG。
//! 块3:Strict 同源策略对真 rg(Arch 包 + Debian 宿主库)硬阻断;SourceRoutedResolver 路由。
//!
//! unix 专有;fixture/delegate 缺失则 skip。
//! prep: bash scripts/prep-complex.sh im && bash scripts/prep-im-delegates.sh

#![cfg(unix)]

use std::path::{Path, PathBuf};

use aevum_cli::{build_with, ingest_closure, open_store, Layout};
use aevum_closure_builder::{
    build_closure_resolved_with_policy, ChainResolver, HostLibResolver, PackageInput,
    PackageLibResolver, Source, SourcePolicy,
};

fn repo_layout() -> Layout {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    Layout::new(repo_root.join(".aevum"))
}

fn run_with_env(
    loader: &Path,
    bin: &Path,
    lib_dirs: &[PathBuf],
    args: &[&str],
    env: &[(&str, String)],
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

// —— 块1+2:外部依赖纳入闭包 + im 真转图 ——
#[test]
fn block12_imagemagick_real_convert() {
    let layout = repo_layout();
    let magick = layout.unpacked_dir("imagemagick").join("usr/bin/magick");
    if !magick.exists() {
        eprintln!("SKIP 块1+2: 未找到 magick,请先 prep-complex.sh im");
        return;
    }
    // delegate 是否装齐(liblcms2 是 libMagickCore 直接 NEEDED,启动必需)。
    if !Path::new("/usr/lib/x86_64-linux-gnu/liblcms2.so.2").exists() {
        eprintln!("SKIP 块1+2: 宿主未装 delegate,请先 `bash scripts/prep-im-delegates.sh`");
        return;
    }

    // 自动推断 runtime_dir + 补闭包(delegate 现在能从宿主解出)。
    let built = build_with(&layout, "imagemagick", Path::new("usr/bin/magick"), &[])
        .expect("build imagemagick");

    // 块1 核心:核心 delegate 现已解入闭包(此前 missing,装齐后 resolved)。
    let lib_names: Vec<&str> = built.libs.iter().map(|(s, _)| s.as_str()).collect();
    for core in ["liblcms2.so", "libjpeg.so", "libpng16.so", "libfreetype.so"] {
        assert!(
            lib_names.iter().any(|s| s.starts_with(core)),
            "核心 delegate {core} 应已解入闭包(装齐宿主后),实得 {lib_names:?}"
        );
    }
    eprintln!(
        "块1: 解 {} 库(核心 delegate 已纳入),仍 missing {} 个(冷门格式 heif/jxl/exr 等): {:?}",
        built.libs.len(),
        built.missing.len(),
        built.missing
    );

    let store = open_store(&layout).expect("open store");
    let ingested = ingest_closure(&store, &built).expect("ingest");

    // 块2 核心:轻隔离真转 PNG。
    let pkg_root = layout.unpacked_dir("imagemagick");
    let im_runtime = pkg_root.join("usr/lib/ImageMagick-7.1.2");
    let coders = im_runtime.join("modules-Q16HDRI/coders");
    let config = im_runtime.join("config-Q16HDRI");
    let loader = {
        let dir = ingested.interpreter_dir.as_ref().expect("应有 loader");
        let name = built
            .interpreter
            .as_ref()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        dir.join(name)
    };
    let out_png = std::env::temp_dir().join(format!("aevum-m4-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&out_png);

    // lib_dirs + ABI 桥接目录(libxml2.so.16→宿主.so.2 的跨源版本号桥接,见 prep-im-delegates.sh)。
    let mut lib_dirs = ingested.lib_dirs.clone();
    let bridge = layout.root.join("abi-bridge");
    if bridge.exists() {
        lib_dirs.push(bridge);
    }

    let out = run_with_env(
        &loader,
        &pkg_root.join("usr/bin/magick"),
        &lib_dirs,
        &["-size", "32x32", "xc:red", out_png.to_str().unwrap()],
        &[
            ("MAGICK_CODER_MODULE_PATH", coders.to_string_lossy().into_owned()),
            ("MAGICK_CONFIGURE_PATH", config.to_string_lossy().into_owned()),
        ],
    )
    .expect("run magick");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "magick 转图应 rc=0,实得 {:?}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(out_png.exists(), "应产出 PNG 文件");
    // 校验 PNG 魔数(真产图非空文件)。
    let bytes = std::fs::read(&out_png).expect("读 PNG");
    assert!(
        bytes.len() > 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n",
        "产出文件应是合法 PNG(魔数校验),前 8 字节: {:?}",
        &bytes[..bytes.len().min(8)]
    );
    eprintln!(
        "块2 通过: magick 轻隔离真转 PNG rc=0,产出 {} 字节合法 PNG(137 coders+delegate dlopen 闭包闭合)",
        bytes.len()
    );
    let _ = std::fs::remove_file(&out_png);
}

// —— 块3:同源硬约束(Strict)对真 rg 阻断 ——
#[test]
fn block3_strict_source_blocks_cross_source() {
    let layout = repo_layout();
    let rg = layout.unpacked_dir("rg").join("usr/bin/rg");
    if !rg.exists() {
        eprintln!("SKIP 块3: 未找到 rg,请先 prep-rg.sh");
        return;
    }
    // rg 是 Arch 包,库(libc 等)从 Debian 宿主取 = 跨源。
    // Lenient:Ok(记诊断);Strict:Err(CrossSource) 硬阻断(PoC-4 铁律落地)。
    let root = layout.unpacked_dir("rg");
    let input = PackageInput {
        name: "rg".into(),
        source: Source::Arch,
        root: root.clone(),
        main_binary: Some(PathBuf::from("usr/bin/rg")),
        runtime_dirs: vec![],
        data_dirs: vec![],
    };
    let resolver = ChainResolver::new(vec![
        Box::new(PackageLibResolver::with_source(
            vec![root.join("usr/lib")],
            Source::Arch,
        )),
        Box::new(HostLibResolver::new()), // Debian 宿主
    ]);

    // Lenient:成功,cross_source 非空。
    let lenient = build_closure_resolved_with_policy(&input, &resolver, SourcePolicy::Lenient)
        .expect("Lenient 应成功");
    assert!(
        !lenient.cross_source.is_empty(),
        "rg 库来自 Debian 宿主,Lenient 应记 cross_source"
    );
    eprintln!(
        "块3 Lenient: 记录 {} 条跨源诊断(如 {:?})",
        lenient.cross_source.len(),
        lenient.cross_source.first().map(|h| &h.soname)
    );

    // Strict:应硬阻断 Err(CrossSource)。
    let strict = build_closure_resolved_with_policy(&input, &resolver, SourcePolicy::Strict);
    assert!(
        strict.is_err(),
        "Strict 模式 rg 库跨源应硬阻断 Err(CrossSource),实得 {strict:?}"
    );
    eprintln!(
        "块3 通过: Strict 模式硬阻断跨源({}) — PoC-4 同源铁律落地",
        strict.unwrap_err()
    );
}
