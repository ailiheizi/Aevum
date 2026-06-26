//! 服务编译器(ADR-0006 阶段4b):纯数据声明 → s6 scandir 服务目录。
//!
//! 设计见 `docs/architecture/bootable/04-init-services-config.md` §3。
//! 把 4a 手写的 `scandir/<svc>/run` 泛化成"从声明生成"。
//!
//! # 路线说明(4b 起步选乙:s6 原生 scandir)
//! Debian trixie 无 s6-rc 包(无编译期依赖图工具)。4b 先用 **s6 原生 scandir**:
//! 每个服务一个目录,内含可执行 `run`;s6-svscan 监督 scandir 下每个服务。
//! `deps`(after/needs)本阶段**记录为元数据**(写进 run 注释 + 单独 deps 文件),
//! 真正的依赖图编排(s6-rc `s6-rc-compile`)留作后续增强(需先解决 s6-rc 来源)。
//!
//! # 零依赖约束
//! 项目 vendor 离线、禁引新 Rust 依赖,且无 `toml` crate。故内置一个**极简
//! TOML 子集解析器**(仅支持服务声明所需:`[section]`、`key = "字符串"`、
//! `key = ["数组"]`、`key = 整数`、`key = true/false`),非通用 TOML。

use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum ServiceError {
    #[error("解析错误(第 {line} 行): {msg}")]
    Parse { line: usize, msg: String },
    #[error("服务声明缺少必填字段: {0}")]
    Missing(&'static str),
    #[error("非法服务类型: {0}(应为 longrun/oneshot)")]
    BadType(String),
    #[error("非法 run.argv: {0}")]
    BadArgv(String),
}

pub type Result<T> = std::result::Result<T, ServiceError>;

/// 服务类型(对齐设计文档 §3.2,scandir 阶段只区分常驻/一次性)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceType {
    /// 常驻进程(s6 监督、退出自动重启)。
    Longrun,
    /// 一次性(跑完即止,如挂载、初始化)。
    Oneshot,
}

/// 一个服务声明(从 TOML 子集解析而来)。
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub name: String,
    pub svc_type: ServiceType,
    pub description: Option<String>,
    /// 启动命令 argv(不是 shell 字符串:避免注入、保持纯数据)。
    pub argv: Vec<String>,
    /// 降权运行的用户(可选)。
    pub user: Option<String>,
    /// 环境变量(有序,确定性)。
    pub env: BTreeMap<String, String>,
    /// 软依赖:这些服务应在本服务**之前**起(scandir 阶段记录为元数据)。
    pub after: Vec<String>,
    /// 硬依赖:这些服务没起来则本服务不应起(scandir 阶段记录为元数据)。
    pub needs: Vec<String>,
}

impl Service {
    /// 从 TOML 子集文本解析一个服务声明。
    pub fn parse(text: &str) -> Result<Service> {
        let doc = parse_toml_subset(text)?;
        let svc = doc.get("service").cloned().unwrap_or_default();
        let run = doc.get("run").cloned().unwrap_or_default();
        let deps = doc.get("deps").cloned().unwrap_or_default();

        let name = svc
            .get_str("name")
            .ok_or(ServiceError::Missing("service.name"))?;
        let svc_type = match svc.get_str("type").as_deref() {
            Some("longrun") | None => ServiceType::Longrun, // 默认 longrun
            Some("oneshot") => ServiceType::Oneshot,
            Some(other) => return Err(ServiceError::BadType(other.to_string())),
        };
        let argv = run.get_arr("argv");
        if argv.is_empty() {
            return Err(ServiceError::BadArgv("run.argv 不能为空".into()));
        }

        Ok(Service {
            name,
            svc_type,
            description: svc.get_str("description"),
            argv,
            user: run.get_str("user"),
            env: run.get_table("env"),
            after: deps.get_arr("after"),
            needs: deps.get_arr("needs"),
        })
    }

    /// 渲染 s6 scandir 服务目录的 `run` 脚本内容。
    ///
    /// 用 busybox sh(平台兜底工具)而非 execline:4a 已验证 busybox 在场且可靠,
    /// 且 execline 语法对 argv 转义另有坑;run 由 argv **逐项 shell 转义**生成,
    /// 不让声明者写 shell(保持"配置即数据")。execline 化作后续优化。
    pub fn render_run(&self, lib_path: &str) -> String {
        let mut s = String::new();
        s.push_str("#!/bin/busybox sh\n");
        if let Some(desc) = &self.description {
            s.push_str(&format!("# {desc}\n"));
        }
        s.push_str(&format!("# aevum-service: {} ({})\n", self.name, self.type_str()));
        if !self.after.is_empty() {
            s.push_str(&format!("# after: {}\n", self.after.join(", ")));
        }
        if !self.needs.is_empty() {
            s.push_str(&format!("# needs: {}\n", self.needs.join(", ")));
        }
        // 库路径(世代自带库,4a 验证过的机制)。
        s.push_str(&format!("export LD_LIBRARY_PATH={lib_path}\n"));
        for (k, v) in &self.env {
            s.push_str(&format!("export {}={}\n", k, sh_quote(v)));
        }
        s.push_str("exec 2>&1\n");
        // 降权:用 s6-applyuidgid(若指定 user)。
        let cmd = self
            .argv
            .iter()
            .map(|a| sh_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        match &self.user {
            Some(u) => s.push_str(&format!("exec s6-applyuidgid -u {} -g {} {cmd}\n", sh_quote(u), sh_quote(u))),
            None => s.push_str(&format!("exec {cmd}\n")),
        }
        s
    }

    fn type_str(&self) -> &'static str {
        match self.svc_type {
            ServiceType::Longrun => "longrun",
            ServiceType::Oneshot => "oneshot",
        }
    }
}

/// 编译产物:scandir 下一个服务目录的相对路径 → 文件内容。
/// 调用方据此在 scandir 真正落盘(引擎/脚本负责 IO + chmod +x run)。
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledService {
    /// 服务名(= scandir 子目录名)。
    pub name: String,
    /// `run` 脚本内容(落盘后须 chmod 0755)。
    pub run: String,
    /// `type` 文件内容(longrun/oneshot,供 s6-rc 未来用 + 自描述)。
    pub type_file: String,
    /// 可选 `dependencies` 文件(after+needs 各一行,供未来 s6-rc / 审计)。
    pub dependencies: Option<String>,
}

/// 把一个服务声明编译成 scandir 服务目录的文件集。
pub fn compile_service(svc: &Service, lib_path: &str) -> CompiledService {
    let mut deps: Vec<String> = Vec::new();
    deps.extend(svc.needs.iter().cloned());
    deps.extend(svc.after.iter().cloned());
    let dependencies = if deps.is_empty() {
        None
    } else {
        let mut uniq: Vec<String> = Vec::new();
        for d in deps {
            if !uniq.contains(&d) {
                uniq.push(d);
            }
        }
        Some(format!("{}\n", uniq.join("\n")))
    };
    CompiledService {
        name: svc.name.clone(),
        run: svc.render_run(lib_path),
        type_file: format!("{}\n", svc.type_str()),
        dependencies,
    }
}

// ───────────────────────── 极简 TOML 子集解析 ─────────────────────────

/// 一个 section 的键值(字符串 / 数组 / 表)。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    strings: BTreeMap<String, String>,
    arrays: BTreeMap<String, Vec<String>>,
    tables: BTreeMap<String, BTreeMap<String, String>>,
}

impl Section {
    /// 取字符串值。
    pub fn get_str(&self, k: &str) -> Option<String> {
        self.strings.get(k).cloned()
    }
    /// 取字符串数组(无则空)。
    pub fn get_arr(&self, k: &str) -> Vec<String> {
        self.arrays.get(k).cloned().unwrap_or_default()
    }
    /// 取内联表(无则空)。
    pub fn get_table(&self, k: &str) -> BTreeMap<String, String> {
        self.tables.get(k).cloned().unwrap_or_default()
    }
    /// 本 section 的所有字符串键(有序)。供调用方遍历未知键(如 etc 文件清单)。
    pub fn string_keys(&self) -> impl Iterator<Item = &String> {
        self.strings.keys()
    }
}

/// 解析 TOML 子集 → section 名 → Section。支持:
/// - `[section]` 头
/// - `key = "字符串"`(双引号,含 `\"`/`\\`/`\n`/`\t` 转义)
/// - `key = ["a", "b"]`(字符串数组,单行)
/// - `key = { a = "x", b = "y" }`(内联表,值为字符串)
/// - `# 注释`、空行
///
/// 不支持嵌套表头/多行数组/数字类型(刻意限范围,保持可控)。
/// 公开供同 workspace 其它 crate(如 etc-builder)复用,避免引入 toml crate(vendor 离线约束)。
pub fn parse_toml_subset(text: &str) -> Result<BTreeMap<String, Section>> {
    let mut doc: BTreeMap<String, Section> = BTreeMap::new();
    let mut cur = String::new(); // 当前 section 名(空 = 顶层,服务声明不用顶层)
    doc.insert(cur.clone(), Section::default());

    for (i, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            cur = name.trim().to_string();
            doc.entry(cur.clone()).or_default();
            continue;
        }
        let eq = line.find('=').ok_or(ServiceError::Parse {
            line: i + 1,
            msg: format!("非 section 非 key=value: {line}"),
        })?;
        // key 可裸写或加引号("含/或.的路径键须 quote",对齐 TOML)。引号 key 剥引号 + 反转义。
        let raw_key = line[..eq].trim();
        let key = parse_string(raw_key).unwrap_or_else(|| raw_key.to_string());
        let val = line[eq + 1..].trim();
        let sec = doc.entry(cur.clone()).or_default();

        if let Some(arr) = val.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            sec.arrays.insert(key, parse_str_array(arr, i + 1)?);
        } else if let Some(tbl) = val.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            sec.tables.insert(key, parse_inline_table(tbl, i + 1)?);
        } else if let Some(s) = parse_string(val) {
            sec.strings.insert(key, s);
        } else {
            return Err(ServiceError::Parse {
                line: i + 1,
                msg: format!("无法解析值(仅支持 \"字符串\"/[数组]/{{表}}): {val}"),
            });
        }
    }
    Ok(doc)
}

/// 去掉行尾注释:`#` 之后丢弃,但**不动双引号字符串内的 #**(否则含 # 的值被截断)。
/// 跟踪引号时跳过 `\"`(转义引号不切换字符串状态)。
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    let mut prev_bs = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' if !prev_bs => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
        prev_bs = c == '\\' && !prev_bs;
    }
    line
}

/// 解析双引号字符串,处理 `\"`(字面引号)和 `\\`(字面反斜杠)转义。非引号包裹返回 None。
fn parse_string(v: &str) -> Option<String> {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        let inner = &v[1..v.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        Some(out)
    } else {
        None
    }
}

/// 解析逗号分隔的字符串数组体(不含外层 []）。
fn parse_str_array(body: &str, line: usize) -> Result<Vec<String>> {
    let body = body.trim();
    if body.is_empty() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for part in split_top_commas(body) {
        let s = parse_string(part.trim()).ok_or(ServiceError::Parse {
            line,
            msg: format!("数组元素须为双引号字符串: {part}"),
        })?;
        out.push(s);
    }
    Ok(out)
}

/// 解析内联表体 `a = "x", b = "y"`(不含外层 {}）。值须为字符串。
fn parse_inline_table(body: &str, line: usize) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    let body = body.trim();
    if body.is_empty() {
        return Ok(map);
    }
    for part in split_top_commas(body) {
        let part = part.trim();
        let eq = part.find('=').ok_or(ServiceError::Parse {
            line,
            msg: format!("内联表项须 key = value: {part}"),
        })?;
        let k = part[..eq].trim().to_string();
        let v = parse_string(part[eq + 1..].trim()).ok_or(ServiceError::Parse {
            line,
            msg: format!("内联表值须为双引号字符串: {part}"),
        })?;
        map.insert(k, v);
    }
    Ok(map)
}

/// 按顶层逗号切分(不切引号内的逗号)。跳过 `\"` 转义引号。
fn split_top_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut prev_bs = false;
    for c in s.chars() {
        match c {
            '"' if !prev_bs => {
                in_str = !in_str;
                cur.push(c);
            }
            ',' if !in_str => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
        prev_bs = c == '\\' && !prev_bs;
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// shell 单引号转义(用于 run 脚本里安全嵌入 argv/env 值)。
/// 单引号内除 ' 外全字面;' 用 '\'' 法拼接。
fn sh_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // 简单值(字母数字 + 安全符号)无需引号。
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "._-/:=".contains(c)) {
        return s.to_string();
    }
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[service]
name = "demo"
type = "longrun"
description = "演示服务"

[run]
argv = ["/usr/bin/foo", "--flag", "value"]
user = "nobody"
env = { PGDATA = "/var/lib/x", LANG = "C" }

[deps]
after = ["network", "mount-data"]
needs = ["mount-data"]
"#;

    #[test]
    fn parse_full_service() {
        let svc = Service::parse(SAMPLE).expect("parse");
        assert_eq!(svc.name, "demo");
        assert_eq!(svc.svc_type, ServiceType::Longrun);
        assert_eq!(svc.description.as_deref(), Some("演示服务"));
        assert_eq!(svc.argv, vec!["/usr/bin/foo", "--flag", "value"]);
        assert_eq!(svc.user.as_deref(), Some("nobody"));
        assert_eq!(svc.env.get("PGDATA").map(String::as_str), Some("/var/lib/x"));
        assert_eq!(svc.env.get("LANG").map(String::as_str), Some("C"));
        assert_eq!(svc.after, vec!["network", "mount-data"]);
        assert_eq!(svc.needs, vec!["mount-data"]);
    }

    #[test]
    fn type_defaults_to_longrun() {
        let svc = Service::parse("[service]\nname=\"x\"\n[run]\nargv=[\"/bin/x\"]\n").unwrap();
        assert_eq!(svc.svc_type, ServiceType::Longrun);
    }

    #[test]
    fn empty_argv_rejected() {
        let err = Service::parse("[service]\nname=\"x\"\n[run]\nargv=[]\n");
        assert!(matches!(err, Err(ServiceError::BadArgv(_))));
    }

    #[test]
    fn missing_name_rejected() {
        let err = Service::parse("[service]\ntype=\"longrun\"\n[run]\nargv=[\"/bin/x\"]\n");
        assert_eq!(err, Err(ServiceError::Missing("service.name")));
    }

    #[test]
    fn bad_type_rejected() {
        let err = Service::parse("[service]\nname=\"x\"\ntype=\"weird\"\n[run]\nargv=[\"/bin/x\"]\n");
        assert!(matches!(err, Err(ServiceError::BadType(_))));
    }

    #[test]
    fn render_run_has_shebang_exec_and_libpath() {
        let svc = Service::parse(SAMPLE).unwrap();
        let run = svc.render_run("/usr/lib");
        assert!(run.starts_with("#!/bin/busybox sh\n"));
        assert!(run.contains("export LD_LIBRARY_PATH=/usr/lib"));
        assert!(run.contains("export LANG=C"));
        assert!(run.contains("exec 2>&1"));
        // 降权 + argv 出现。
        assert!(run.contains("s6-applyuidgid -u nobody -g nobody"));
        assert!(run.contains("/usr/bin/foo --flag value"));
    }

    #[test]
    fn render_run_no_user_plain_exec() {
        let svc = Service::parse("[service]\nname=\"x\"\n[run]\nargv=[\"/bin/x\",\"a b\"]\n").unwrap();
        let run = svc.render_run("/lib");
        assert!(!run.contains("s6-applyuidgid"));
        // 含空格的参数被引号包裹(注入安全)。
        assert!(run.contains("exec /bin/x 'a b'"), "got:\n{run}");
    }

    #[test]
    fn compile_emits_files() {
        let svc = Service::parse(SAMPLE).unwrap();
        let c = compile_service(&svc, "/usr/lib");
        assert_eq!(c.name, "demo");
        assert_eq!(c.type_file, "longrun\n");
        assert!(c.run.contains("exec"));
        // needs 在前、after 去重后续上。
        let deps = c.dependencies.unwrap();
        assert!(deps.contains("mount-data"));
        assert!(deps.contains("network"));
    }

    #[test]
    fn sh_quote_handles_specials() {
        assert_eq!(sh_quote("simple"), "simple");
        assert_eq!(sh_quote("/usr/bin/x"), "/usr/bin/x");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn comments_and_blank_lines_ignored() {
        let t = "# 头注释\n\n[service]\nname = \"c\"  # 行尾注释\n[run]\nargv = [\"/b\"]\n";
        let svc = Service::parse(t).unwrap();
        assert_eq!(svc.name, "c");
    }

    #[test]
    fn string_with_hash_not_truncated() {
        // 值里的 # 不应被当注释截断。
        let t = "[service]\nname = \"c\"\n[run]\nargv = [\"/bin/x\", \"a#b\"]\n";
        let svc = Service::parse(t).unwrap();
        assert_eq!(svc.argv, vec!["/bin/x", "a#b"]);
    }

    #[test]
    fn escaped_quotes_in_argv() {
        // argv 元素含转义双引号 + 逗号 + #(shell 片段常见),应正确解析。
        let t = "[service]\nname = \"c\"\n[run]\nargv = [\"/bin/sh\", \"-c\", \"echo \\\"hi, #1\\\"\"]\n";
        let svc = Service::parse(t).unwrap();
        assert_eq!(svc.argv, vec!["/bin/sh", "-c", "echo \"hi, #1\""]);
    }

    #[test]
    fn quoted_keys_stripped() {
        // 含 / 或 . 的键须 quote;解析应剥引号(供 etc [files] 用路径键)。
        let doc = parse_toml_subset("[files]\n\"ssh/banner\" = \"hi\"\nmotd = \"m\"\n").unwrap();
        let files = doc.get("files").unwrap();
        assert_eq!(files.get_str("ssh/banner").as_deref(), Some("hi"));
        assert_eq!(files.get_str("motd").as_deref(), Some("m"));
    }
}
