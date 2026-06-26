//! Foundation manifest 解析(判据3 的数据源)。
//!
//! 平台维护、签名保护的核心包清单(见 `docs/layers/01-foundation.md` §2)。
//! verify 据此校验:required 包必须在场、版本精确匹配(§4.1/4.2),
//! 且 manifest 里的包视为"foundation 提供",消除闭合性误报。
//!
//! # 格式(本实现采用的 TOML 子集)
//! 文档 §2 原设计用 `[[packages]]` 数组表头,但本项目的零依赖 TOML 解析器
//! ([`aevum_service_compiler::parse_toml_subset`])不支持数组表头。故改用
//! **每包一个 `[foundation.<name>]` 分节**(语义等价,解析器原生支持):
//!
//! ```toml
//! [meta]
//! version = "1.0.0"
//! channel = "stable"
//!
//! [foundation.init]
//! version = "1.2.0"
//! required = "true"          # 注:解析器只认字符串,布尔写成 "true"/"false"
//! upgrade_policy = "on-major"
//!
//! [foundation.solver-core]
//! version = "0.9.0"
//! required = "true"
//! ```
//!
//! 签名链(§2.1)与启动期校验(§3)本轮不做(vendor 离线无 ed25519 crate),标注待办。

use aevum_service_compiler::parse_toml_subset;
use std::collections::BTreeMap;

/// 一个 foundation 核心包的清单条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundationPackage {
    pub name: String,
    /// 精确版本(verify 判据3 要求 candidate 同名包版本与此精确匹配)。
    pub version: String,
    /// 是否必装(required 包缺失 → verify 失败)。
    pub required: bool,
    /// 升级策略(always / on-major / manual);本轮仅保留,不参与 verify 判定。
    pub upgrade_policy: String,
}

/// Foundation manifest:平台核心包清单。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundationManifest {
    /// manifest 自身版本(`[meta] version`),诊断/审计用。
    pub manifest_version: String,
    /// 核心包(按包名有序)。
    pub packages: Vec<FoundationPackage>,
}

/// 解析错误。
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest TOML 解析失败: {0}")]
    Toml(String),
    #[error("foundation 包 {name} 缺 version 字段")]
    MissingVersion { name: String },
}

impl FoundationManifest {
    /// 解析 manifest TOML(`[foundation.<name>]` 分节形态,见模块文档)。
    ///
    /// `required` 字段:`"true"`(默认缺省也视为 true,因 foundation 包默认必装)以外的值视为 false。
    pub fn parse(text: &str) -> Result<FoundationManifest, ManifestError> {
        let doc = parse_toml_subset(text).map_err(|e| ManifestError::Toml(e.to_string()))?;

        let manifest_version = doc
            .get("meta")
            .and_then(|s| s.get_str("version"))
            .unwrap_or_default();

        let mut packages = Vec::new();
        for (section_name, sec) in &doc {
            // 只认 `foundation.<name>` 分节。
            let Some(pkg_name) = section_name.strip_prefix("foundation.") else {
                continue;
            };
            let pkg_name = pkg_name.trim();
            if pkg_name.is_empty() {
                continue;
            }
            let version = sec.get_str("version").ok_or_else(|| ManifestError::MissingVersion {
                name: pkg_name.to_string(),
            })?;
            // required 缺省视为 true(foundation 包默认必装);显式 "false" 才非必装。
            let required = sec
                .get_str("required")
                .map(|v| v != "false")
                .unwrap_or(true);
            let upgrade_policy = sec.get_str("upgrade_policy").unwrap_or_else(|| "manual".into());
            packages.push(FoundationPackage {
                name: pkg_name.to_string(),
                version,
                required,
                upgrade_policy,
            });
        }
        // BTreeMap 迭代已按 section 名有序,但 strip 前缀后顺序仍稳定;显式再排一次保确定性。
        packages.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(FoundationManifest { manifest_version, packages })
    }

    /// 全部 foundation 包名(供 verify 并入 `foundation_provided`,消除闭合性误报)。
    pub fn provided_names(&self) -> Vec<String> {
        self.packages.iter().map(|p| p.name.clone()).collect()
    }

    /// 包名 → 期望版本映射(供判据3 版本精确匹配)。
    pub fn version_map(&self) -> BTreeMap<&str, &str> {
        self.packages.iter().map(|p| (p.name.as_str(), p.version.as_str())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[meta]
version = "1.0.0"
channel = "stable"

[foundation.init]
version = "1.2.0"
required = "true"
upgrade_policy = "on-major"

[foundation.solver-core]
version = "0.9.0"
required = "true"

[foundation.optional-tool]
version = "2.0.0"
required = "false"
"#;

    #[test]
    fn parse_extracts_packages() {
        let m = FoundationManifest::parse(SAMPLE).unwrap();
        assert_eq!(m.manifest_version, "1.0.0");
        assert_eq!(m.packages.len(), 3);
        // 有序:foundation.init / foundation.optional-tool / foundation.solver-core
        let init = m.packages.iter().find(|p| p.name == "init").unwrap();
        assert_eq!(init.version, "1.2.0");
        assert!(init.required);
        assert_eq!(init.upgrade_policy, "on-major");
    }

    #[test]
    fn required_defaults_true_explicit_false_respected() {
        let m = FoundationManifest::parse(SAMPLE).unwrap();
        let opt = m.packages.iter().find(|p| p.name == "optional-tool").unwrap();
        assert!(!opt.required, "显式 required=false 应非必装");
        let solver = m.packages.iter().find(|p| p.name == "solver-core").unwrap();
        assert!(solver.required);
        assert_eq!(solver.upgrade_policy, "manual", "缺省 upgrade_policy 应为 manual");
    }

    #[test]
    fn provided_names_and_version_map() {
        let m = FoundationManifest::parse(SAMPLE).unwrap();
        let names = m.provided_names();
        assert!(names.contains(&"init".to_string()));
        assert!(names.contains(&"solver-core".to_string()));
        let vmap = m.version_map();
        assert_eq!(vmap.get("init"), Some(&"1.2.0"));
    }

    #[test]
    fn missing_version_errors() {
        let bad = "[foundation.broken]\nrequired = \"true\"\n";
        assert!(matches!(
            FoundationManifest::parse(bad),
            Err(ManifestError::MissingVersion { .. })
        ));
    }

    #[test]
    fn empty_manifest_ok() {
        let m = FoundationManifest::parse("[meta]\nversion = \"1.0\"\n").unwrap();
        assert!(m.packages.is_empty());
    }
}
