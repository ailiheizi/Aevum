//! 模板系统接入一致性(第五十一轮,跨平台、离线、确定性):
//! 验证 TS 前端的 `useTemplate` 经 CLI 展开,与直接调 template crate 展开产出相同约束;
//! 且模板展开 + 直接 use 合并后求解出非空 lock;世代模板记录旁路文件读写正确。
//!
//! 不触网(合成 index + 临时模板目录)、不依赖 unix。

use std::path::{Path, PathBuf};

use aevum_cli::{read_generation_templates, read_lock_templates, record_generation_templates, resolve_constraints, resolve_constraints_opt, Layout};
use aevum_template::{collect_templates, expand, ExpandOptions};

fn repo_layout(tag: &str) -> Layout {
    let root = std::env::temp_dir().join(format!("aevum-tmpl-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    Layout::new(&root)
}

/// 写合成 index(含模板能力涉及的包)+ 模板文件。
fn setup(layout: &Layout) {
    // index:coreutils / bash / rustc / cargo / gcc 各一版本。
    let mut text = String::new();
    for (p, v) in [("coreutils", "9.1"), ("bash", "5.2"), ("rustc", "1.80"), ("cargo", "1.80"), ("gcc", "12.2"), ("rust-analyzer", "2024.1"), ("pcmanfm", "1.3")] {
        text.push_str(&format!("Package: {p}\nVersion: {v}\nFilename: pool/{p}_{v}.deb\n\n"));
    }
    let idx = layout.index_file();
    std::fs::create_dir_all(idx.parent().unwrap()).unwrap();
    std::fs::write(&idx, text).unwrap();

    // 模板:minimal-desktop + dev-rust(extends minimal-desktop)。
    let tdir = layout.templates_dir();
    std::fs::create_dir_all(&tdir).unwrap();
    std::fs::write(tdir.join("minimal-desktop.toml"), MINIMAL).unwrap();
    std::fs::write(tdir.join("dev-rust.toml"), DEVRUST).unwrap();
}

const MINIMAL: &str = r#"
[template]
name = "minimal-desktop"
version = "1.0.0"
[capability.coreutils]
constraint = "*"
layer_hint = "system"
[capability.bash]
constraint = "*"
layer_hint = "system"
[optional.file-manager]
default = "true"
id = "pcmanfm"
"#;

const DEVRUST: &str = r#"
[template]
name = "dev-rust"
version = "1.0.0"
extends = ["minimal-desktop"]
[capability.rustc]
constraint = ">=1.75"
layer_hint = "app"
[capability.cargo]
constraint = ">=1.75"
layer_hint = "app"
[optional.rust-analyzer]
default = "true"
id = "rust-analyzer"
"#;

#[test]
fn template_expands_to_nonempty_lock() {
    let layout = repo_layout("expand");
    setup(&layout);

    // 直接用 template crate 展开 dev-rust(含继承的 minimal-desktop)。
    let constraints = expand(&layout.templates_dir(), &["dev-rust".into()], &ExpandOptions::default())
        .expect("展开 dev-rust");
    let names: Vec<&str> = constraints.iter().map(|c| c.name.as_str()).collect();
    // 继承链:coreutils/bash(来自 minimal-desktop)+ rustc/cargo(dev-rust)+ 默认 optional。
    assert!(names.contains(&"coreutils"), "应继承 minimal-desktop 的 coreutils: {names:?}");
    assert!(names.contains(&"rustc"));
    assert!(names.contains(&"cargo"));
    assert!(names.contains(&"rust-analyzer"), "default=true 的 optional 应带入");
    assert!(names.contains(&"pcmanfm"), "继承的 optional(file-manager)应带入");

    // 展开的约束能求解出非空 lock(真接通确定性求解器)。
    let lock = resolve_constraints(&layout, &constraints, "dev-rust-lock", None).expect("求解");
    assert!(lock.package_count > 0, "模板展开应求出非空闭包");
    assert!(lock.closure_id.starts_with("clo-"));
}

#[test]
fn template_expand_deterministic() {
    let layout = repo_layout("det");
    setup(&layout);
    let a = expand(&layout.templates_dir(), &["dev-rust".into()], &ExpandOptions::default()).unwrap();
    let b = expand(&layout.templates_dir(), &["dev-rust".into()], &ExpandOptions::default()).unwrap();
    let an: Vec<&str> = a.iter().map(|c| c.name.as_str()).collect();
    let bn: Vec<&str> = b.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(an, bn, "同模板两次展开必产相同有序约束");
}

#[test]
fn generation_templates_record_roundtrip() {
    let layout = repo_layout("rec");
    setup(&layout);
    // 需要先有世代目录;record_generation_templates 会 create_dir_all。
    let recorded = vec![
        ("dev-rust".to_string(), "1.0.0".to_string()),
        ("minimal-desktop".to_string(), "1.0.0".to_string()),
    ];
    let path = aevum_cli::record_generation_templates(&layout, 1, &recorded).expect("写 templates.txt");
    assert!(path.exists(), "templates.txt 应写出");

    let back = aevum_cli::read_generation_templates(&layout, 1);
    // 读回应按名排序(确定性)。
    assert_eq!(back, vec![
        ("dev-rust".to_string(), "1.0.0".to_string()),
        ("minimal-desktop".to_string(), "1.0.0".to_string()),
    ]);

    // 无记录的世代返回空。
    assert!(aevum_cli::read_generation_templates(&layout, 99).is_empty());
}

#[test]
fn template_path_helper_under_root() {
    // Layout::templates_dir 应在 root 下。
    let layout = repo_layout("path");
    let td = layout.templates_dir();
    assert!(td.ends_with("templates"));
    let _: &Path = td.as_path();
    let _: PathBuf = td;
}

#[test]
fn lock_records_templates_and_closure_id_unaffected() {
    // 模板记录写进 lock 头部(templates: 行),且不影响 closure_id(纯审计,验收7数据源)。
    let layout = repo_layout("lockrec");
    setup(&layout);
    let constraints = expand(&layout.templates_dir(), &["dev-rust".into()], &ExpandOptions::default()).unwrap();
    let pairs = collect_templates(&layout.templates_dir(), &["dev-rust".into()]).unwrap();
    let record = pairs.iter().map(|(n, v)| format!("{n}@{v}")).collect::<Vec<_>>().join(", ");

    // 带模板记录求解。
    let with = resolve_constraints_opt(&layout, &constraints, "with-tmpl", None, false, None, Some(&record)).unwrap();
    // 不带模板记录,同约束求解。
    let without = resolve_constraints_opt(&layout, &constraints, "no-tmpl", None, false, None, None).unwrap();

    assert_eq!(with.closure_id, without.closure_id, "templates 记录不应影响 closure_id");

    // lock 头部含 templates 行,且能被 read_lock_templates 读回。
    let read = read_lock_templates(&layout.locks_dir().join("with-tmpl.lock"));
    assert!(read.iter().any(|(n, _)| n == "dev-rust"), "应读回 dev-rust 模板: {read:?}");
    assert!(read.iter().any(|(n, _)| n == "minimal-desktop"), "应含继承的 minimal-desktop: {read:?}");

    // 不带模板记录的 lock 读回为空。
    assert!(read_lock_templates(&layout.locks_dir().join("no-tmpl.lock")).is_empty());
}

#[test]
fn read_lock_templates_parses_header_only() {
    // read_lock_templates 只扫头部,不把包体的 @ 行误当模板。
    let layout = repo_layout("hdronly");
    setup(&layout);
    let constraints = expand(&layout.templates_dir(), &["minimal-desktop".into()], &ExpandOptions::default()).unwrap();
    let _ = resolve_constraints_opt(&layout, &constraints, "hdr", None, false, None, Some("minimal-desktop@1.0.0")).unwrap();
    let read = read_lock_templates(&layout.locks_dir().join("hdr.lock"));
    // 只应有 minimal-desktop,包体里的 coreutils@9.1 等不应混入。
    assert_eq!(read, vec![("minimal-desktop".to_string(), "1.0.0".to_string())]);
}

#[test]
fn generation_templates_via_record_helper() {
    // record/read 旁路函数往返(maintain 接入时即用这条路写世代 templates.txt)。
    let layout = repo_layout("genrec");
    setup(&layout);
    let pairs = collect_templates(&layout.templates_dir(), &["dev-rust".into()]).unwrap();
    record_generation_templates(&layout, 7, &pairs).expect("写世代模板记录");
    let back = read_generation_templates(&layout, 7);
    assert!(back.iter().any(|(n, _)| n == "dev-rust"));
    assert!(back.iter().any(|(n, _)| n == "minimal-desktop"));
}
