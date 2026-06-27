//! ELF 解析:抽取动态链接所需的运行时信息。
//!
//! 被 `closure-builder` 用于补闭包。参照 PoC-2/4/5 的 Python 解析器,
//! 但生产用 `goblin` 而非手写(PoC 手写仅为零依赖验证)。
//!
//! # PoC 铁律(来自 PoC-5,照直觉写会崩)
//! 复杂包(python 77 扩展、imagemagick 137 插件)靠运行时 `dlopen`,
//! 补闭包不能只递归主二进制的 `DT_NEEDED`。因此本 crate 既提供单文件解析
//! ([`parse_file`]),也提供全包 ELF 扫描([`scan_dir`])——后者是补闭包正确性的关键。

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ElfError {
    #[error("读取 ELF 文件失败: {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("解析 ELF 失败: {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: goblin::error::Error,
    },
    #[error("不是 ELF 文件: {0}")]
    NotElf(PathBuf),
}

/// 单个 ELF 文件抽取出的动态链接信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElfInfo {
    /// 该 ELF 自身路径。
    pub path: PathBuf,
    /// `PT_INTERP`:动态链接器路径(可执行文件才有,如 `/lib64/ld-linux-x86-64.so.2`)。
    pub interpreter: Option<String>,
    /// `DT_NEEDED`:直接依赖的共享库 soname 列表(如 `libc.so.6`)。
    pub needed: Vec<String>,
    /// `DT_SONAME`:本库对外暴露的名字(用于把 NEEDED 名解析到具体文件)。
    pub soname: Option<String>,
    /// `DT_RPATH` / `DT_RUNPATH`:库搜索路径(可能含 `$ORIGIN`,解析时需展开)。
    pub runpaths: Vec<String>,
    /// 是否是动态可执行 / 动态库(静态二进制无需补库)。
    pub is_dynamic: bool,
}

/// 解析单个 ELF 文件。非 ELF 返回 [`ElfError::NotElf`],调用方在扫描全包时应跳过它。
pub fn parse_file(path: impl AsRef<Path>) -> Result<ElfInfo, ElfError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| ElfError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_bytes(path, &bytes)
}

/// 从已读入的字节解析(便于测试 / 避免重复 IO)。
pub fn parse_bytes(path: impl AsRef<Path>, bytes: &[u8]) -> Result<ElfInfo, ElfError> {
    let path = path.as_ref();
    // 快速魔数判断,非 ELF 直接拒绝(扫全包时大量普通文件会走到这里)。
    if bytes.len() < 4 || &bytes[0..4] != b"\x7fELF" {
        return Err(ElfError::NotElf(path.to_path_buf()));
    }
    let elf = goblin::elf::Elf::parse(bytes).map_err(|source| ElfError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    let info = ElfInfo {
        path: path.to_path_buf(),
        interpreter: elf.interpreter.map(String::from),
        needed: elf.libraries.iter().map(|s| s.to_string()).collect(),
        soname: elf.soname.map(String::from),
        runpaths: collect_runpaths(&elf),
        is_dynamic: elf.is_lib || elf.interpreter.is_some() || !elf.libraries.is_empty(),
    };
    Ok(info)
}

/// 从 dynamic 段收集 RPATH 与 RUNPATH(goblin 已把字符串解析好,合并去重保序)。
fn collect_runpaths(elf: &goblin::elf::Elf) -> Vec<String> {
    let mut out = Vec::new();
    for rp in elf.rpaths.iter().chain(elf.runpaths.iter()) {
        for entry in rp.split(':').filter(|s| !s.is_empty()) {
            let e = entry.to_string();
            if !out.contains(&e) {
                out.push(e);
            }
        }
    }
    out
}

/// 扫描一个目录下的全部 ELF 文件(递归)。
///
/// # PoC-5 铁律
/// 补闭包必须扫**全包所有 ELF**(插件/扩展通过 dlopen 加载,不在主二进制 NEEDED 里),
/// 否则 python/imagemagick 这类包运行时崩。非 ELF 文件被静默跳过。
///
/// # PoC-5 铁律(符号链接)
/// 用 [`std::fs::symlink_metadata`] 判断,**不解引用符号链接**——复杂包大量用软链
/// (137 个工具软链到 magick),解引用会爆量重复 + 破坏布局。本扫描只处理真实文件。
pub fn scan_dir(root: impl AsRef<Path>) -> Result<Vec<ElfInfo>, ElfError> {
    let root = root.as_ref();
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|source| ElfError::Io {
            path: dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| ElfError::Io {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            // 不解引用:软链单独看其自身类型,不跟进目标。
            let meta = std::fs::symlink_metadata(&path).map_err(|source| ElfError::Io {
                path: path.clone(),
                source,
            })?;
            let ft = meta.file_type();
            if ft.is_symlink() {
                continue; // 软链不重复解析,其目标会作为真实文件被单独扫到
            }
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                match parse_file(&path) {
                    Ok(info) => out.push(info),
                    Err(ElfError::NotElf(_)) => {} // 普通文件,跳过
                    Err(e) => return Err(e),
                }
            }
        }
    }
    // 确定性顺序(扫描受文件系统顺序影响,排序后稳定)。
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_elf() {
        let err = parse_bytes("/tmp/x", b"not an elf file").unwrap_err();
        assert!(matches!(err, ElfError::NotElf(_)));
    }

    #[test]
    fn rejects_too_short() {
        let err = parse_bytes("/tmp/x", b"\x7fEL").unwrap_err();
        assert!(matches!(err, ElfError::NotElf(_)));
    }

    // ── P1-8:正向断言 DT_NEEDED/DT_SONAME/PT_INTERP 抽取 + scan_dir 只返真 ELF ──
    // 用 cc 在测试期编译真 ELF(无 cc 则跳过,同 milestone7 的 have() 模式);
    // 不 check-in 二进制 blob,fixture 来自真工具链 → 真正校验 goblin 解析而非手搓字节。

    #[cfg(unix)]
    fn have_cc() -> bool {
        std::process::Command::new("cc")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[cfg(unix)]
    fn cc_compile(dir: &std::path::Path, src_name: &str, src: &str, args: &[&str]) -> std::path::PathBuf {
        let src_path = dir.join(src_name);
        std::fs::write(&src_path, src).unwrap();
        let out = dir.join(src_name.replace(".c", ".out"));
        let status = std::process::Command::new("cc")
            .arg(&src_path)
            .args(args)
            .arg("-o")
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success(), "cc 编译 {src_name} 失败");
        out
    }

    #[cfg(unix)]
    #[test]
    fn extracts_needed_soname_from_real_so() {
        if !have_cc() {
            eprintln!("SKIP extracts_needed_soname: 无 cc");
            return;
        }
        let dir = std::env::temp_dir().join(format!("elf-needed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 依赖 libm 的共享库,带 soname。
        let so = cc_compile(
            &dir,
            "m.c",
            "#include <math.h>\ndouble g(double x){return sqrt(x)+cos(x);}\n",
            &["-shared", "-fPIC", "-lm", "-Wl,-soname,libm_user.so.1"],
        );
        let info = parse_file(&so).unwrap();
        assert!(info.is_dynamic, "共享库应是 dynamic");
        assert!(info.needed.iter().any(|n| n.starts_with("libm.so")), "DT_NEEDED 应含 libm.so.*: {:?}", info.needed);
        assert_eq!(info.soname.as_deref(), Some("libm_user.so.1"), "DT_SONAME 应抽取");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn extracts_interpreter_from_real_exe() {
        if !have_cc() {
            eprintln!("SKIP extracts_interpreter: 无 cc");
            return;
        }
        let dir = std::env::temp_dir().join(format!("elf-interp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = cc_compile(&dir, "main.c", "int main(){return 0;}\n", &[]);
        let info = parse_file(&exe).unwrap();
        // 动态可执行有 PT_INTERP(ld-linux...)。
        assert!(info.interpreter.is_some(), "动态可执行应有 PT_INTERP");
        assert!(info.interpreter.as_deref().unwrap().contains("ld-"), "interpreter 应是 ld-linux: {:?}", info.interpreter);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn scan_dir_returns_only_real_elfs_skipping_nonelf_and_symlink() {
        if !have_cc() {
            eprintln!("SKIP scan_dir: 无 cc");
            return;
        }
        let dir = std::env::temp_dir().join(format!("elf-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 2 个真 ELF(共享库 + 可执行)。
        cc_compile(&dir, "a.c", "int a(){return 1;}\n", &["-shared", "-fPIC", "-Wl,-soname,liba.so"]);
        cc_compile(&dir, "main.c", "int main(){return 0;}\n", &[]);
        // 1 个非 ELF 文件。
        std::fs::write(dir.join("readme.txt"), b"not an elf").unwrap();
        // 1 个 symlink 指向真 ELF(scan_dir 应跳过 symlink 本身,不重复解析)。
        std::os::unix::fs::symlink(dir.join("a.out"), dir.join("a-link.so")).unwrap();

        let infos = scan_dir(&dir).unwrap();
        // 恰好 2 个真 ELF(非 ELF 跳过、symlink 跳过)。
        assert_eq!(infos.len(), 2, "scan_dir 应只返 2 个真 ELF(跳非 ELF + symlink),实得 {:?}", infos.iter().map(|i| &i.path).collect::<Vec<_>>());
        // 确定性排序。
        assert!(infos.windows(2).all(|w| w[0].path <= w[1].path), "应有序");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 真实复杂包(rg/python)的 dlopen 全包闭包测试仍在 WSL 用 poc fixture 跑(里程碑2 验收)。
}
