//! 世代(generation):创建 / 原子切换 / 瞬时回滚 / 可达性 GC。
//!
//! Aevum 三大核心卖点,PoC-7 已用真文件 + 真 symlink 验证(切换 0.09ms、回滚 0.095ms、
//! GC 不误删共享依赖)。本 crate 直译自 `poc/poc7-core-mechanics/experiment.py`。
//! 参照设计:`docs/architecture/foundations/02-generation.md`、
//! `docs/architecture/runtime/01-generation-lifecycle.md`。
//!
//! # PoC-7 铁律
//! - active 切换用 **symlink + rename**(POSIX rename 原子)。
//! - 回滚 = active 指针回指旧世代,**不重建**(亚毫秒级)。
//! - GC 用**可达性**:保留 active/历史世代引用的 hash,共享 hash 不误删。
//!
//! # 平台说明
//! symlink + rename 的原子语义是 unix 专有。非 unix 平台返回
//! [`GenError::Unsupported`],骨架仍可 `cargo build`(真实测试在 WSL/真 Linux)。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub mod bootloader;

#[derive(Debug, Error)]
pub enum GenError {
    #[error("IO 错误 at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("世代不存在: {0}")]
    NotFound(u64),
    #[error("当前平台不支持原子 symlink 切换。请在 Linux/WSL 运行")]
    Unsupported,
}

type Result<T> = std::result::Result<T, GenError>;

/// 世代管理器:`root/` 下含 `gen-NNN/` 各世代与 `active` 指针(symlink)。
pub struct GenerationManager {
    root: PathBuf,
}

/// 一个包在世代中的引用:逻辑名 + 它在 store 中的对象目录名(`<hash>-<name>`)。
#[derive(Debug, Clone)]
pub struct PackageRef {
    pub name: String,
    /// store 对象目录的绝对路径(world:symlink 将指向它)。
    pub store_dir: PathBuf,
    /// store 对象目录名,写入 lock.txt 供 GC 可达性分析。
    pub object_id: String,
    /// 该对象在系统布局中的相对路径(如 `usr/bin/hello`、`usr/lib/libc.so.6`)。
    /// 世代据此建**层级** symlink,忠实保留布局(供 bootroot 重建)。
    /// `None` 退化为用 `name` 作扁平路径(兼容简单场景/旧测试)。
    pub rel_path: Option<PathBuf>,
}

impl GenerationManager {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|source| GenError::Io {
            path: root.clone(),
            source,
        })?;
        Ok(GenerationManager { root })
    }

    fn gen_dir(&self, id: u64) -> PathBuf {
        self.root.join(format!("gen-{id:03}"))
    }

    /// 公开世代目录路径(`root/gen-NNN`)。供上层在世代下挂旁路产物
    /// (如 repair 方案C 的 `private-views/<app>/` 私有依赖视图)。世代是否存在不校验,仅算路径。
    pub fn generation_dir(&self, id: u64) -> PathBuf {
        self.gen_dir(id)
    }

    fn active_link(&self) -> PathBuf {
        self.root.join("active")
    }

    /// 造一个世代:`gen-NNN/packages/<name>` → symlink 到 store 对象目录,
    /// 并写 `lock.txt`(每行一个 object_id,供 GC)。对应 PoC 的 `make_generation`。
    ///
    /// 原子构建(P1-7):在**临时目录** `.gen-NNN.tmp.<pid>` 里建 `packages/` + `lock.txt`,
    /// 全部完成后原子 rename 到 `gen-NNN`。旧实现原地建,SIGKILL/磁盘满 中途留下
    /// 半填充的 `gen-NNN`(packages 不全、lock.txt 缺失或残缺),却被后续 set_active/
    /// refresh_profile/verify 当作完整世代用 → 缺文件、GC 漏算对象。现在 gen-NNN 只在
    /// rename 那一刻整体出现。重建已存在世代时:先建好临时目录,再删旧的、rename 覆盖。
    pub fn make_generation(&self, id: u64, packages: &[PackageRef]) -> Result<PathBuf> {
        let g = self.gen_dir(id);
        // 临时构建目录(与 gen 目录同父,保证 rename 在同一文件系统、原子)。
        let tmp = self
            .root
            .join(format!(".gen-{id:03}.tmp.{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp); // 清掉上次中断的同名残留
        let pkgdir = tmp.join("packages");
        std::fs::create_dir_all(&pkgdir).map_err(|source| GenError::Io {
            path: pkgdir.clone(),
            source,
        })?;
        let mut refs = Vec::new();
        for p in packages {
            // 用 rel_path 建层级布局(usr/bin/hello),无则退回扁平 name。
            let rel = p.rel_path.clone().unwrap_or_else(|| PathBuf::from(&p.name));
            let link = pkgdir.join(&rel);
            if let Some(parent) = link.parent() {
                std::fs::create_dir_all(parent).map_err(|source| GenError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            // 临时目录里不会有同名残留(已 remove_dir_all),但层级布局下同一父目录
            // 可能被多个 ref 复用,故仍按需删旧。
            if link.exists() || is_symlink(&link) {
                std::fs::remove_file(&link).map_err(|source| GenError::Io {
                    path: link.clone(),
                    source,
                })?;
            }
            if let Err(e) = symlink(&p.store_dir, &link) {
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(e);
            }
            refs.push(p.object_id.clone());
        }
        let lock = tmp.join("lock.txt");
        if let Err(source) = std::fs::write(&lock, refs.join("\n")) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(GenError::Io { path: lock, source });
        }

        // 提交点:原子 rename 临时目录 → gen-NNN。重建时先删旧世代(rename 不能覆盖非空目录)。
        if g.exists() {
            std::fs::remove_dir_all(&g).map_err(|source| GenError::Io {
                path: g.clone(),
                source,
            })?;
        }
        if let Err(source) = std::fs::rename(&tmp, &g) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(GenError::Io { path: g.clone(), source });
        }
        Ok(g)
    }

    /// 原子切换 active 指针:写临时 symlink → rename 覆盖。对应 PoC 的 `set_active`。
    ///
    /// # PoC-7
    /// POSIX `rename` 对同文件系统内的替换是原子的;切换后系统立刻看到新世代内容。
    pub fn set_active(&self, id: u64) -> Result<()> {
        let g = self.gen_dir(id);
        if !g.exists() {
            return Err(GenError::NotFound(id));
        }
        let active = self.active_link();
        // 临时名用 pid(不用随机/时钟,保持确定性约束)。
        let tmp = self.root.join(format!(".active.tmp.{}", std::process::id()));
        if tmp.exists() || is_symlink(&tmp) {
            std::fs::remove_file(&tmp).map_err(|source| GenError::Io {
                path: tmp.clone(),
                source,
            })?;
        }
        symlink(&g, &tmp)?;
        // 原子替换(rename 覆盖已存在的 symlink)。
        std::fs::rename(&tmp, &active).map_err(|source| GenError::Io {
            path: active.clone(),
            source,
        })?;
        Ok(())
    }

    /// 瞬时回滚:把 active 指回某历史世代,不重建。对应 PoC 的 B 段。
    /// 本质就是 [`set_active`],单列以表达语义(回滚不做求解/构建)。
    pub fn rollback(&self, id: u64) -> Result<()> {
        self.set_active(id)
    }

    /// 读 active 指向的世代 id(从 symlink 目标名解析)。
    pub fn active_generation(&self) -> Result<Option<u64>> {
        let active = self.active_link();
        if !is_symlink(&active) {
            return Ok(None);
        }
        let target = std::fs::read_link(&active).map_err(|source| GenError::Io {
            path: active.clone(),
            source,
        })?;
        let name = target.file_name().and_then(|s| s.to_str()).unwrap_or("");
        Ok(name.strip_prefix("gen-").and_then(|n| n.parse().ok()))
    }

    /// 枚举一个世代引用的对象:递归读 `gen-NNN/packages/` 下层级 symlink
    /// → `(rel_path, store 对象目录)`,rel_path 是相对 packages/ 的布局路径(如 `usr/bin/hello`)。
    ///
    /// 让外部(如导出 bootroot)从**真实世代**读取内容 + **布局**——而非脚本另行拼装。
    /// 这是"Aevum 引擎驱动引导内容"的关键:bootroot 内容与布局都源自世代。
    pub fn generation_refs(&self, id: u64) -> Result<Vec<(PathBuf, PathBuf)>> {
        let pkgdir = self.gen_dir(id).join("packages");
        if !pkgdir.exists() {
            return Err(GenError::NotFound(id));
        }
        let mut out = Vec::new();
        // 递归:目录则进入,symlink(指向 store 对象)则记 (相对rel_path, 目标)。
        let mut stack = vec![pkgdir.clone()];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir).map_err(|source| GenError::Io {
                path: dir.clone(),
                source,
            })?;
            for e in entries {
                let e = e.map_err(|source| GenError::Io {
                    path: dir.clone(),
                    source,
                })?;
                let path = e.path();
                if is_symlink(&path) {
                    let store_dir = std::fs::read_link(&path).map_err(|source| GenError::Io {
                        path: path.clone(),
                        source,
                    })?;
                    let rel = path.strip_prefix(&pkgdir).unwrap_or(&path).to_path_buf();
                    out.push((rel, store_dir));
                } else if path.is_dir() {
                    stack.push(path);
                }
            }
        }
        // 确定性顺序。
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    /// 读一个世代引用的全部 object_id(`<hash>-<name>` 列表)。
    ///
    /// 合并两个来源:
    /// - `lock.txt`:世代共享布局(`packages/`)的对象(make_generation 写)。
    /// - `private-objects.txt`:repair 方案C 私有视图(`private-views/<app>/`)引用的对象
    ///   (attach_keep_two_views 写);**必须纳入 GC 可达性**,否则私有视图对象会被误回收。
    ///
    /// 供 GC 可达性分析与 verify 完整性校验复用。两文件都不存在返回空 Vec。结果去重。
    pub fn generation_object_ids(&self, id: u64) -> Result<Vec<String>> {
        let dir = self.gen_dir(id);
        let mut seen = std::collections::BTreeSet::new();
        for fname in ["lock.txt", "private-objects.txt"] {
            let f = dir.join(fname);
            if !f.exists() {
                continue;
            }
            let text = std::fs::read_to_string(&f).map_err(|source| GenError::Io {
                path: f.clone(),
                source,
            })?;
            for line in text.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    seen.insert(line.to_string());
                }
            }
        }
        Ok(seen.into_iter().collect())
    }

    /// 可达性 GC:保留 `keep_gen_ids` 世代 lock.txt 引用的 object_id,回收 store 中其余对象。
    /// 对应 PoC 的 `gc`。返回 (删除的, 保留的)。
    ///
    /// # PoC-7
    /// 多世代共享 store hash:删一个世代后,被其它世代仍引用的 hash 不能删(不误删共享依赖)。
    /// 调用方负责实际删 store 目录;本函数只做可达性计算与待删清单,避免与 store crate 强耦合。
    pub fn compute_garbage(&self, keep_gen_ids: &[u64], all_objects: &[String]) -> Result<GcPlan> {
        let mut referenced: HashSet<String> = HashSet::new();
        for &gid in keep_gen_ids {
            for oid in self.generation_object_ids(gid)? {
                referenced.insert(oid);
            }
        }
        let all: HashSet<String> = all_objects.iter().cloned().collect();
        let mut garbage: Vec<String> = all.difference(&referenced).cloned().collect();
        let mut kept: Vec<String> = referenced.into_iter().collect();
        garbage.sort();
        kept.sort();
        Ok(GcPlan { garbage, kept })
    }
}

/// GC 计算结果:待回收对象 + 仍被引用的对象。
#[derive(Debug, PartialEq, Eq)]
pub struct GcPlan {
    pub garbage: Vec<String>,
    pub kept: Vec<String>,
}

// ---------- 平台相关:symlink 与判断 ----------

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// 创建符号链接。unix 用 `std::os::unix::fs::symlink`;非 unix 返回 Unsupported。
fn symlink(target: &Path, link: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|source| GenError::Io {
            path: link.to_path_buf(),
            source,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(GenError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 可达性 GC 是纯计算,不依赖 symlink,可在任意平台测(PoC-7 C 段的核心断言)。
    #[test]
    fn gc_does_not_remove_shared() {
        let root = std::env::temp_dir().join(format!("aevum-gen-gc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mgr = GenerationManager::open(&root).unwrap();

        // 手工写两个世代的 lock.txt:gen-1 用 {py311, libc};gen-2 用 {py312, libc}。
        for (id, objs) in [(1u64, ["py311-python", "libc-libc"]), (2, ["py312-python", "libc-libc"])] {
            let g = mgr.gen_dir(id).join("packages");
            std::fs::create_dir_all(&g).unwrap();
            std::fs::write(mgr.gen_dir(id).join("lock.txt"), objs.join("\n")).unwrap();
        }

        let all = vec![
            "py311-python".to_string(),
            "py312-python".to_string(),
            "libc-libc".to_string(),
        ];
        // 只保留 gen-1:py312 应回收,libc(共享)与 py311 保留。
        let plan = mgr.compute_garbage(&[1], &all).unwrap();
        assert_eq!(plan.garbage, vec!["py312-python".to_string()]);
        assert!(plan.kept.contains(&"libc-libc".to_string()), "共享 libc 不能误删");
        assert!(plan.kept.contains(&"py311-python".to_string()));

        let _ = std::fs::remove_dir_all(&root);
    }

    // 原子切换 / 回滚的真实测试(symlink+rename)在 WSL/真 Linux 跑——
    // 直译 PoC-7 A/B 段:set_active(g1)→读到旧内容,set_active(g2)→读到新内容,
    // rollback(g1)→又读回旧内容。NTFS symlink 会失败,故不在 Windows 断言(见 CLAUDE.md)。
    #[cfg(unix)]
    #[test]
    fn atomic_switch_and_rollback() {
        let root = std::env::temp_dir().join(format!("aevum-gen-sw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mgr = GenerationManager::open(&root).unwrap();

        // 两个假 store 对象目录
        let store = root.join("store");
        let d1 = store.join("h1-python");
        let d2 = store.join("h2-python");
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();

        let g1 = vec![PackageRef { name: "python".into(), store_dir: d1.clone(), object_id: "h1-python".into(), rel_path: None }];
        let g2 = vec![PackageRef { name: "python".into(), store_dir: d2.clone(), object_id: "h2-python".into(), rel_path: None }];
        mgr.make_generation(1, &g1).unwrap();
        mgr.make_generation(2, &g2).unwrap();

        mgr.set_active(1).unwrap();
        assert_eq!(mgr.active_generation().unwrap(), Some(1));
        mgr.set_active(2).unwrap();
        assert_eq!(mgr.active_generation().unwrap(), Some(2));
        mgr.rollback(1).unwrap();
        assert_eq!(mgr.active_generation().unwrap(), Some(1));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn generation_refs_enumerates_world() {
        // 从真实世代枚举包 + store 路径(bootroot 内容的引擎来源)。
        let root = std::env::temp_dir().join(format!("aevum-gen-refs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mgr = GenerationManager::open(&root).unwrap();
        let store = root.join("store");
        let da = store.join("ha-busybox");
        let db = store.join("hb-hello");
        std::fs::create_dir_all(&da).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        let pkgs = vec![
            PackageRef { name: "busybox".into(), store_dir: da.clone(), object_id: "ha-busybox".into(), rel_path: Some(PathBuf::from("usr/bin/busybox")) },
            PackageRef { name: "hello".into(), store_dir: db.clone(), object_id: "hb-hello".into(), rel_path: Some(PathBuf::from("usr/bin/hello")) },
        ];
        mgr.make_generation(5, &pkgs).unwrap();

        let refs = mgr.generation_refs(5).unwrap();
        assert_eq!(refs.len(), 2);
        // 布局被忠实保留:rel_path 是层级路径,确定性排序
        assert_eq!(refs[0].0, std::path::PathBuf::from("usr/bin/busybox"));
        assert_eq!(refs[1].0, std::path::PathBuf::from("usr/bin/hello"));
        // symlink 目标解析回 store 对象目录
        assert!(refs[0].1.ends_with("ha-busybox"));
        assert!(refs[1].1.ends_with("hb-hello"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // P1-7:原子构建 —— rebuild 正确替换、提交后无残留临时目录、lock.txt 与 packages 同时就绪。
    #[cfg(unix)]
    #[test]
    fn make_generation_atomic_rebuild_no_temp_leftover() {
        let root = std::env::temp_dir().join(format!("aevum-gen-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mgr = GenerationManager::open(&root).unwrap();
        let store = root.join("store");
        let d1 = store.join("h1-foo");
        let d2 = store.join("h2-bar");
        std::fs::create_dir_all(&d1).unwrap();
        std::fs::create_dir_all(&d2).unwrap();

        // 首建:gen-9 = {foo}。
        let g1 = vec![PackageRef { name: "foo".into(), store_dir: d1.clone(), object_id: "h1-foo".into(), rel_path: Some(PathBuf::from("usr/bin/foo")) }];
        let gdir = mgr.make_generation(9, &g1).unwrap();
        assert!(gdir.join("packages/usr/bin/foo").exists(), "首建后 foo symlink 应在");
        assert!(gdir.join("lock.txt").exists(), "lock.txt 应与 packages 一并就绪");
        assert_eq!(std::fs::read_to_string(gdir.join("lock.txt")).unwrap(), "h1-foo");

        // 重建同 id:gen-9 = {bar}。必须整体替换(旧 foo 不残留)。
        let g2 = vec![PackageRef { name: "bar".into(), store_dir: d2.clone(), object_id: "h2-bar".into(), rel_path: Some(PathBuf::from("usr/bin/bar")) }];
        mgr.make_generation(9, &g2).unwrap();
        assert!(gdir.join("packages/usr/bin/bar").exists(), "重建后 bar 应在");
        assert!(!gdir.join("packages/usr/bin/foo").exists(), "重建后旧 foo 不应残留");
        assert_eq!(std::fs::read_to_string(gdir.join("lock.txt")).unwrap(), "h2-bar");

        // 关键:提交后 root 下无任何 .gen-*.tmp.* 残留临时目录。
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".gen-"))
            .collect();
        assert!(leftovers.is_empty(), "不应有残留临时构建目录: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
