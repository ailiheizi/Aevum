//! 里程碑7 端到端验收:接通 resolve→install(真下载 → 解包 → 入 store → 造世代)。
//!
//! 补上最大断桥:lock 里的包能从 Debian 镜像真下载、SHA256 校验、解包入 store、造世代激活。
//! 用真实小包 `hello`(53KB,Debian 经典最小包)`--only` 单装,证明桥能通。
//!
//! unix 专有(ar/tar 解包 + symlink 世代);需 curl+ar+tar+网络,缺则 skip。
//! 前提:`bash scripts/prep-index.sh`(真实 Debian 索引)。

#![cfg(unix)]

use std::path::PathBuf;

use aevum_cli::{install, resolve, Layout, DEFAULT_MIRROR};

fn repo_layout() -> Layout {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    // 用独立子目录,避免与其它里程碑的 .aevum 状态互扰。
    Layout::new(repo_root.join(".aevum"))
}

fn have(tool: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn milestone7_install_hello_from_mirror() {
    let layout = repo_layout();
    if !layout.index_file().exists() {
        eprintln!("SKIP milestone7: 未找到索引,请先 `bash scripts/prep-index.sh`");
        return;
    }
    if !have("curl") || !have("ar") || !have("tar") || !have("xz") {
        eprintln!("SKIP milestone7: 缺 curl/ar/tar/xz(.deb 的 data.tar.xz 需 xz)");
        return;
    }

    // 1. resolve hello → lock(含 filename + sha256)。
    let lock = resolve(&layout, &["hello".to_string()], "m7-hello").expect("resolve hello");
    let hello = lock
        .locked
        .iter()
        .find(|p| p.name == "hello")
        .expect("lock 应含 hello");
    assert!(!hello.filename.is_empty(), "hello 应有 filename(下载用)");
    assert!(
        hello.fingerprint.starts_with("sha256:"),
        "hello fingerprint 应是 sha256: {}",
        hello.fingerprint
    );
    eprintln!("resolve: hello {} filename={}", hello.version, hello.filename);

    // 2. install --only hello:真下载 .deb → SHA256 校验 → 解包入 store → 造 gen-7。
    let report = match install(&layout, &lock, DEFAULT_MIRROR, &["hello".to_string()], 7) {
        Ok(r) => r,
        Err(e) => {
            let msg = e.to_string();
            // 只对下载/网络类错误 skip(真外部依赖);解包/校验失败是真 bug,必须暴露。
            if msg.contains("下载失败") || msg.contains("curl") {
                eprintln!("SKIP milestone7: 网络/镜像不可达: {e}");
                return;
            }
            panic!("install 失败(非网络,真 bug): {e}");
        }
    };

    assert!(report.installed.contains(&"hello".to_string()), "应装了 hello");
    assert_eq!(report.generation, 7);
    assert!(report.store_objects > 0, "store 应有对象");
    eprintln!(
        "install: 装 {:?} → gen-{} ({} store 对象)",
        report.installed, report.generation, report.store_objects
    );

    // 3. 验证真装进去了:解包目录里应有 hello 二进制(/usr/bin/hello)。
    let hello_bin = layout.unpacked_dir("hello").join("usr/bin/hello");
    assert!(
        hello_bin.exists(),
        "解包后应有 hello 二进制 {hello_bin:?}(SHA256 已校验 = 内容来自镜像未被污染)"
    );

    // 4. 世代已激活,store 有对象。
    let gens = aevum_cli::open_generations(&layout).expect("open generations");
    assert_eq!(gens.active_generation().unwrap(), Some(7), "gen-7 应已激活");

    eprintln!(
        "里程碑7 达成: resolve→install 全链打通 — hello 从 Debian 镜像真下载、\
         SHA256 内容寻址校验、解包入 store、造世代激活。包管理器核心链路成立。"
    );
}
