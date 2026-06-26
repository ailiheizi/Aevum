//! /etc 系统配置基底生成器(ADR-0006 阶段4c)。
//!
//! 设计见 `docs/architecture/bootable/04-init-services-config.md` §4。
//! 把纯数据 TOML 系统配置编译成 `/etc` **基底文件树**(只读、内容寻址、随世代走)。
//! 运行时与可变层(overlayfs upper)合并;基底随世代回退,可变层按 runtime/04 处理。
//!
//! 本 crate 只管"声明 → 基底文件内容";落盘 + overlay 挂载由 CLI/init 负责。
//!
//! # 零依赖
//! 复用 `aevum-service-compiler` 的极简 TOML 子集解析器(项目 vendor 离线、无 toml crate)。

use std::collections::BTreeMap;
use thiserror::Error;

use aevum_service_compiler::parse_toml_subset;

#[derive(Debug, Error, PartialEq)]
pub enum EtcError {
    #[error("TOML 解析: {0}")]
    Parse(String),
    #[error("非法 /etc 相对路径(不可含 .. 或绝对路径): {0}")]
    BadPath(String),
}

type Result<T> = std::result::Result<T, EtcError>;

/// 一个 /etc 基底文件:相对 `/etc` 的路径 + 内容。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EtcFile {
    /// 相对 `/etc` 的路径(如 `hostname`、`locale.conf`、`ssh/sshd_config`)。
    pub rel_path: String,
    /// 文件内容。
    pub content: String,
}

/// 编译结果:/etc 基底文件集(确定性有序,便于内容寻址)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EtcBase {
    pub files: Vec<EtcFile>,
}

/// 校验 /etc 相对路径:非空、非绝对、不含 `..` 段(防逃逸出 /etc)。
fn check_rel(path: &str) -> Result<()> {
    if path.is_empty() || path.starts_with('/') {
        return Err(EtcError::BadPath(path.to_string()));
    }
    if path.split('/').any(|seg| seg == ".." || seg == ".") {
        return Err(EtcError::BadPath(path.to_string()));
    }
    Ok(())
}

/// 从 TOML 系统配置编译 /etc 基底。
///
/// 支持的 section:
/// - `[system]`:`hostname`/`locale`/`timezone` → 生成对应标准 /etc 文件。
/// - `[files]`:内联表 `"相对路径" = "内容"`,逐项生成任意 /etc 文件。
///
/// 标准字段映射:
/// - hostname → `/etc/hostname`(内容为主机名 + 换行)
/// - locale   → `/etc/locale.conf`(`LANG=<locale>`)
/// - timezone → `/etc/timezone`
pub fn build_etc(toml: &str) -> Result<EtcBase> {
    let doc = parse_toml_subset(toml).map_err(|e| EtcError::Parse(e.to_string()))?;
    let mut map: BTreeMap<String, String> = BTreeMap::new();

    if let Some(sys) = doc.get("system") {
        if let Some(h) = sys.get_str("hostname") {
            map.insert("hostname".into(), format!("{h}\n"));
        }
        if let Some(l) = sys.get_str("locale") {
            map.insert("locale.conf".into(), format!("LANG={l}\n"));
        }
        if let Some(tz) = sys.get_str("timezone") {
            map.insert("timezone".into(), format!("{tz}\n"));
        }
    }

    // [files]:任意 /etc 文件(相对路径 → 内容)。section 下每个 `"路径" = "内容"`。
    if let Some(files) = doc.get("files") {
        for k in files.string_keys() {
            if let Some(v) = files.get_str(k) {
                check_rel(k)?;
                map.insert(k.clone(), v);
            }
        }
    }

    Ok(EtcBase {
        files: map
            .into_iter()
            .map(|(rel_path, content)| EtcFile { rel_path, content })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_fields_map_to_standard_files() {
        let toml = r#"
[system]
hostname = "aevum-box"
locale = "C.UTF-8"
timezone = "UTC"
"#;
        let base = build_etc(toml).unwrap();
        let get = |p: &str| base.files.iter().find(|f| f.rel_path == p).map(|f| f.content.clone());
        assert_eq!(get("hostname").as_deref(), Some("aevum-box\n"));
        assert_eq!(get("locale.conf").as_deref(), Some("LANG=C.UTF-8\n"));
        assert_eq!(get("timezone").as_deref(), Some("UTC\n"));
    }

    #[test]
    fn arbitrary_files_section() {
        let toml = "[files]\nmotd = \"Welcome to Aevum\"\n\"ssh/banner\" = \"hi\"\n";
        let base = build_etc(toml).unwrap();
        let get = |p: &str| base.files.iter().find(|f| f.rel_path == p).map(|f| f.content.clone());
        assert_eq!(get("motd").as_deref(), Some("Welcome to Aevum"));
        assert_eq!(get("ssh/banner").as_deref(), Some("hi"));
    }

    #[test]
    fn deterministic_order() {
        let toml = "[files]\nz = \"1\"\na = \"2\"\nm = \"3\"\n";
        let base = build_etc(toml).unwrap();
        let paths: Vec<&str> = base.files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["a", "m", "z"], "应按路径有序(内容寻址确定性)");
    }

    #[test]
    fn rejects_path_escape() {
        assert!(matches!(build_etc("[files]\n\"../evil\" = \"x\"\n"), Err(EtcError::BadPath(_))));
        assert!(matches!(build_etc("[files]\n\"/abs\" = \"x\"\n"), Err(EtcError::BadPath(_))));
    }

    #[test]
    fn empty_config_empty_base() {
        assert_eq!(build_etc("").unwrap(), EtcBase::default());
    }

    #[test]
    fn combined_system_and_files() {
        let toml = "[system]\nhostname = \"h\"\n[files]\nmotd = \"m\"\n";
        let base = build_etc(toml).unwrap();
        assert_eq!(base.files.len(), 2);
    }
}
