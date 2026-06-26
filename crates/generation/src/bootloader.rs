//! bootloader 多世代菜单(ADR-0006 阶段3 收尾)。
//!
//! 把 syslinux/extlinux 菜单的**渲染**和 **DEFAULT(active 指针)改写**从脚本
//! here-doc 提进引擎,让 `switch`/`rollback` 不只改世代 `active` symlink,还能
//! 同步改开机菜单的默认项——回滚成为真命令,而非"重造镜像"模拟。
//!
//! 设计要点:
//! - 纯字符串 + 文件 IO,不引依赖、不依赖 unix 语义(syslinux.cfg 是文本)。
//!   因此可在任意平台单测(对齐 crate 的跨平台测试约定)。
//! - DEFAULT 改写是**只动一行**的原子写(临时文件 + rename),不重渲整张菜单,
//!   语义上等价世代 `active` 指针回指:不重建、不碰各 LABEL。
//! - LABEL 名规范 `gen<id>`,与 set_default 解析互逆。

use std::path::{Path, PathBuf};

/// 一个可引导世代在菜单里的条目。
#[derive(Debug, Clone)]
pub struct BootEntry {
    /// 世代 id。
    pub generation: u64,
    /// 该世代的 initramfs 文件名(FAT 根内,如 `initrd-50.gz`)。
    pub initrd: String,
}

/// 多世代菜单。`default` 为开机默认引导的世代 id(= active 指针)。
#[derive(Debug, Clone)]
pub struct BootMenu {
    /// 默认引导(active)世代 id。
    pub default: u64,
    /// 共享内核文件名(FAT 根内,如 `vmlinuz`)。
    pub kernel: String,
    /// 自动引导倒计时(单位:1/10 秒,syslinux TIMEOUT 语义)。
    pub timeout: u32,
    /// 各世代条目(顺序即菜单显示顺序)。
    pub entries: Vec<BootEntry>,
    /// 内核命令行追加(APPEND)。
    pub append: String,
}

impl BootMenu {
    /// LABEL 名:`gen<id>`,与 [`parse_default`] 互逆。
    fn label(id: u64) -> String {
        format!("gen{id}")
    }

    /// 渲染成 syslinux.cfg 文本。等价原脚本 here-doc,但由引擎产出。
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("DEFAULT {}\n", Self::label(self.default)));
        s.push_str("PROMPT 1\n");
        s.push_str(&format!("TIMEOUT {}\n", self.timeout));
        s.push_str("UI menu.c32\n");
        s.push_str("MENU TITLE Aevum - 选择世代 (generation)\n");
        for e in &self.entries {
            let active = if e.generation == self.default {
                " (active/default)"
            } else {
                ""
            };
            s.push('\n');
            s.push_str(&format!("LABEL {}\n", Self::label(e.generation)));
            s.push_str(&format!(
                "  MENU LABEL Aevum generation {}{}\n",
                e.generation, active
            ));
            s.push_str(&format!("  KERNEL /{}\n", self.kernel));
            s.push_str(&format!("  INITRD /{}\n", e.initrd));
            s.push_str(&format!("  APPEND {}\n", self.append));
        }
        s
    }

    /// 渲染并写入 syslinux.cfg(覆盖)。
    pub fn write_to(&self, cfg: &Path) -> std::io::Result<()> {
        std::fs::write(cfg, self.render())
    }
}

/// 从 syslinux.cfg 文本解析当前 DEFAULT 指向的世代 id。
/// 找 `DEFAULT gen<NN>` 行,解析出 `<NN>`。无法解析返回 None。
pub fn parse_default(cfg_text: &str) -> Option<u64> {
    for line in cfg_text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("DEFAULT ") {
            let label = rest.trim();
            if let Some(num) = label.strip_prefix("gen") {
                if let Ok(id) = num.trim().parse::<u64>() {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// 只改 DEFAULT 行指向新世代(active 指针回指),不重渲其余菜单。
///
/// 这是 `switch`/`rollback` 对 bootloader 的动作:语义上等价改世代 `active`
/// symlink,但作用在开机菜单上。原子写(临时文件 + rename)。
///
/// 要求菜单里已存在 `LABEL gen<id>` 条目;否则返回 Err(改了也引导不起来)。
pub fn set_default(cfg: &Path, generation: u64) -> std::io::Result<()> {
    let text = std::fs::read_to_string(cfg)?;
    let want_label = BootMenu::label(generation);

    // 校验目标 LABEL 存在(避免把 DEFAULT 指向不存在的世代)。
    let has_label = text
        .lines()
        .any(|l| l.trim().strip_prefix("LABEL ").map(str::trim) == Some(want_label.as_str()));
    if !has_label {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("菜单中无 LABEL {want_label}(该世代未在可引导菜单里)"),
        ));
    }

    // 改 DEFAULT 行;同时刷新各 MENU LABEL 的 "(active/default)" 标记。
    let mut out = String::with_capacity(text.len());
    let mut pending_label: Option<u64> = None; // 上一行 LABEL 解析出的世代
    for line in text.lines() {
        let t = line.trim();
        if t.strip_prefix("DEFAULT ").is_some() {
            out.push_str(&format!("DEFAULT {want_label}\n"));
            continue;
        }
        // 记录 LABEL 行解析出的世代,供随后的 MENU LABEL 行判定标记。
        if let Some(lab) = t.strip_prefix("LABEL ") {
            pending_label = lab.trim().strip_prefix("gen").and_then(|n| n.trim().parse().ok());
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if let Some(rest) = t.strip_prefix("MENU LABEL ") {
            // 去掉旧的 active 标记,再按新 default 重加。
            let base = rest
                .strip_suffix(" (active/default)")
                .unwrap_or(rest);
            let mark = if pending_label == Some(generation) {
                " (active/default)"
            } else {
                ""
            };
            // 保留原行的前导缩进。
            let indent = &line[..line.len() - line.trim_start().len()];
            out.push_str(&format!("{indent}MENU LABEL {base}{mark}\n"));
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    // 原子写:临时文件 + rename(同目录)。
    let tmp = cfg.with_extension(format!("cfg.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, cfg)?;
    Ok(())
}

/// 在给定目录下找 syslinux 配置文件(syslinux.cfg / extlinux.conf)。
/// 返回第一个存在的路径,都不存在返回 None。
pub fn find_config(dir: &Path) -> Option<PathBuf> {
    for name in ["syslinux.cfg", "extlinux.conf"] {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_menu() -> BootMenu {
        BootMenu {
            default: 50,
            kernel: "vmlinuz".into(),
            timeout: 30,
            entries: vec![
                BootEntry { generation: 50, initrd: "initrd-50.gz".into() },
                BootEntry { generation: 51, initrd: "initrd-51.gz".into() },
            ],
            append: "console=ttyS0 rdinit=/init panic=1".into(),
        }
    }

    #[test]
    fn render_has_default_and_labels() {
        let cfg = sample_menu().render();
        assert!(cfg.contains("DEFAULT gen50"));
        assert!(cfg.contains("LABEL gen50"));
        assert!(cfg.contains("LABEL gen51"));
        assert!(cfg.contains("INITRD /initrd-50.gz"));
        assert!(cfg.contains("KERNEL /vmlinuz"));
        // 默认项带标记,非默认项不带。
        assert!(cfg.contains("Aevum generation 50 (active/default)"));
        assert!(cfg.contains("Aevum generation 51\n"));
    }

    #[test]
    fn render_roundtrips_through_parse_default() {
        let cfg = sample_menu().render();
        assert_eq!(parse_default(&cfg), Some(50));
    }

    #[test]
    fn set_default_switches_pointer_and_marker() {
        let dir = std::env::temp_dir().join(format!("aevum-boot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("syslinux.cfg");
        sample_menu().write_to(&cfg).unwrap();

        // 回滚:DEFAULT 50 → 51。
        set_default(&cfg, 51).unwrap();
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(parse_default(&text), Some(51));
        // active 标记跟着移到 51,50 不再带标记。
        assert!(text.contains("Aevum generation 51 (active/default)"));
        assert!(text.contains("Aevum generation 50\n"));
        // 各 LABEL/INITRD 条目原样保留(不重建)。
        assert!(text.contains("INITRD /initrd-50.gz"));
        assert!(text.contains("INITRD /initrd-51.gz"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_default_rejects_unknown_generation() {
        let dir = std::env::temp_dir().join(format!("aevum-boot-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("syslinux.cfg");
        sample_menu().write_to(&cfg).unwrap();
        // gen-99 不在菜单 → 拒绝(否则 DEFAULT 指向不存在世代,开机失败)。
        let err = set_default(&cfg, 99);
        assert!(err.is_err());
        // 原 DEFAULT 未被破坏。
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(parse_default(&text), Some(50));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
