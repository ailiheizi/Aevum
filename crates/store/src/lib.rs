//! 内容寻址存储(content-addressed store)。
//!
//! 内容 → sha256 → `store/<hash>-<name>/`,不可变、加载期校验。
//! 参照设计:`docs/architecture/foundations/01-store.md`。
//!
//! # PoC-6 铁律(照天真 read→write 复制会丢语义,务必遵守)
//! setuid/setgid/sticky/可执行位是**语义**,不是装饰:
//! - 纳入哈希输入(权限不同 → 不同对象);
//! - 入库/取出时显式恢复(`PermissionsExt`),否则 sudo 等提权二进制失效。
//!
//! # PoC-5 铁律(符号链接)
//! 符号链接**保留不解引用**——复杂包大量用软链(137 个工具软链到 magick),
//! 解引用会爆量复制 + 破坏布局。入库时记录 link target,取出时重建 symlink。
//!
//! # 平台说明
//! 权限位与 symlink 是 unix 语义。真实行为在 WSL/真 Linux 验证;
//! 非 unix 平台(如开发用 Windows)相关操作返回 [`StoreError::Unsupported`],
//! 使骨架仍可 `cargo build`(NTFS 不支持这些语义,见 CLAUDE.md)。

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("IO 错误 at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("内容校验失败: 期望 {expected}, 实得 {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("store 中不存在对象: {0}")]
    NotFound(String),
    #[error("当前平台不支持该 unix 语义操作(权限位/symlink): {0}。请在 Linux/WSL 运行")]
    Unsupported(&'static str),
}

type Result<T> = std::result::Result<T, StoreError>;

/// 一个内容寻址 store,根目录下每个对象是 `<hash>-<name>/` 目录。
pub struct Store {
    root: PathBuf,
}

/// 单个文件入库时纳入哈希的语义元数据(PoC-6:权限位是哈希输入)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    /// unix 权限位(含 setuid/setgid/sticky/可执行),低 12 位有意义。
    pub mode: u32,
    /// 是否符号链接(PoC-5:保留不解引用)。
    pub is_symlink: bool,
}

/// [`Store::ingest_dir`] 入库的单个条目:保留相对布局 + GC 用的 object_id。
#[derive(Debug, Clone)]
pub struct IngestedEntry {
    /// 相对 ingest 根目录的路径(保留包内布局)。
    pub rel_path: PathBuf,
    /// store 对象目录名 `<hash>-<name>`,写入世代 lock 供 GC 可达性分析。
    pub object_id: String,
    /// store 对象目录绝对路径。
    pub store_dir: PathBuf,
    /// 该条目的语义元数据。
    pub meta: FileMeta,
}

impl Store {
    /// 打开/创建一个 store。
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|source| StoreError::Io {
            path: root.clone(),
            source,
        })?;
        Ok(Store { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 计算单个 blob 的内容哈希(取 sha256 前 12 hex,与 PoC-7 `put` 一致)。
    ///
    /// # PoC-6
    /// 权限位 `mode` 纳入哈希输入——同内容不同权限 = 不同对象。
    pub fn hash_blob(content: &[u8], meta: FileMeta) -> String {
        let mut h = Sha256::new();
        h.update(content);
        // 权限语义进哈希:不同 mode 必须得到不同 hash。
        h.update(meta.mode.to_le_bytes());
        h.update([meta.is_symlink as u8]);
        let digest = hex::encode(h.finalize());
        digest[..12].to_string()
    }

    /// 把单个内容寻址入库,返回对象目录路径。
    ///
    /// 对应 PoC-7 的 `put`,但加上 PoC-6 的权限位恢复:写文件后显式 `set_permissions`。
    pub fn put(&self, name: &str, content: &[u8], meta: FileMeta) -> Result<PathBuf> {
        let hash = Self::hash_blob(content, meta);
        let dir = self.root.join(format!("{hash}-{name}"));
        std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
            path: dir.clone(),
            source,
        })?;
        let file = dir.join(name);
        std::fs::write(&file, content).map_err(|source| StoreError::Io {
            path: file.clone(),
            source,
        })?;
        // PoC-6:显式恢复权限位,否则 setuid/可执行语义丢失。
        restore_mode(&file, meta.mode)?;
        Ok(dir)
    }

    /// 取出对象目录,并做加载期内容校验(PoC 设计:不可变 + 加载期校验)。
    ///
    /// 重算对象内文件的内容哈希(含权限位/symlink 语义),与目录名里的 `hash` 比对;
    /// 失配返回 [`StoreError::HashMismatch`](内容被篡改或位腐败)。
    ///
    /// 内容校验依赖真实 unix mode(哈希输入含权限位),故仅在 unix 生效;
    /// 非 unix 平台只校验对象存在(无 mode 语义,无法重算,见 CLAUDE.md)。
    pub fn get(&self, hash: &str, name: &str) -> Result<PathBuf> {
        let dir = self.root.join(format!("{hash}-{name}"));
        if !dir.exists() {
            return Err(StoreError::NotFound(format!("{hash}-{name}")));
        }
        #[cfg(unix)]
        {
            let file = dir.join(name);
            let meta = read_meta(&file)?;
            // symlink 对象:内容 = link target 字节(与 put_symlink 入库时一致)。
            let content = if meta.is_symlink {
                read_link_bytes(&file)?
            } else {
                std::fs::read(&file).map_err(|source| StoreError::Io {
                    path: file.clone(),
                    source,
                })?
            };
            let actual = Self::hash_blob(&content, meta);
            if actual != hash {
                return Err(StoreError::HashMismatch {
                    expected: hash.to_string(),
                    actual,
                });
            }
        }
        Ok(dir)
    }

    /// 列出 store 中所有对象目录名(`<hash>-<name>`)。GC 用(见 generation crate)。
    pub fn list_objects(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&self.root).map_err(|source| StoreError::Io {
            path: self.root.clone(),
            source,
        })?;
        for e in entries {
            let e = e.map_err(|source| StoreError::Io {
                path: self.root.clone(),
                source,
            })?;
            if e.path().is_dir() {
                out.push(e.file_name().to_string_lossy().into_owned());
            }
        }
        out.sort();
        Ok(out)
    }

    /// 把一个符号链接内容寻址入库(PoC-5:保留不解引用)。
    ///
    /// symlink 的"内容"= 其 target 路径字节;hash 纳入 `is_symlink=true` 以与同名普通文件区分。
    /// 入库时用 `os::unix::fs::symlink` 重建链接(不复制目标),取出时布局原样保留。
    ///
    /// unix 专有;非 unix 平台返回 [`StoreError::Unsupported`]。
    pub fn put_symlink(&self, name: &str, target: &Path, meta: FileMeta) -> Result<PathBuf> {
        let content = target.as_os_str().as_encoded_bytes();
        let hash = Self::hash_blob(content, meta);
        let dir = self.root.join(format!("{hash}-{name}"));
        std::fs::create_dir_all(&dir).map_err(|source| StoreError::Io {
            path: dir.clone(),
            source,
        })?;
        let link = dir.join(name);
        #[cfg(unix)]
        {
            // 已存在先删(内容寻址下同 hash 即同内容,幂等)。
            if link.exists() || std::fs::symlink_metadata(&link).is_ok() {
                let _ = std::fs::remove_file(&link);
            }
            std::os::unix::fs::symlink(target, &link).map_err(|source| StoreError::Io {
                path: link.clone(),
                source,
            })?;
            Ok(dir)
        }
        #[cfg(not(unix))]
        {
            let _ = link;
            Err(StoreError::Unsupported("put_symlink"))
        }
    }

    /// 把一整个包目录内容寻址入库(PoC-5/6),返回每个条目的相对布局 + object_id。
    ///
    /// - 遍历用 `symlink_metadata` **不解引用**符号链接(PoC-5):符号链接走 [`Self::put_symlink`]
    ///   保留,不跟进目标(复杂包大量软链,解引用会爆量复制 + 破坏布局)。
    /// - 普通文件读真实 mode 纳入哈希并恢复(PoC-6),**不硬编码权限**。
    /// - 遍历顺序排序,保证确定性。
    ///
    /// 注:里程碑1 的 rg 闭环是离散文件集合(逐个 put),不走此函数;此函数为里程碑2
    /// 的运行时目录/数据目录整体入库准备,并满足设计的 ingest 语义。
    pub fn ingest_dir(&self, root: impl AsRef<Path>) -> Result<Vec<IngestedEntry>> {
        let root = root.as_ref();
        let mut out = Vec::new();
        // 收集所有条目(确定性顺序),目录本身不入库(只入库文件/symlink)。
        let mut stack = vec![root.to_path_buf()];
        let mut files: Vec<PathBuf> = Vec::new();
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir).map_err(|source| StoreError::Io {
                path: dir.clone(),
                source,
            })?;
            for e in entries {
                let e = e.map_err(|source| StoreError::Io {
                    path: dir.clone(),
                    source,
                })?;
                let path = e.path();
                let m = std::fs::symlink_metadata(&path).map_err(|source| StoreError::Io {
                    path: path.clone(),
                    source,
                })?;
                let ft = m.file_type();
                if ft.is_symlink() {
                    files.push(path); // symlink 不跟进,作为条目入库
                } else if ft.is_dir() {
                    stack.push(path);
                } else if ft.is_file() {
                    files.push(path);
                }
            }
        }
        files.sort();

        for path in files {
            let rel_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let meta = read_meta(&path)?;
            let store_dir = if meta.is_symlink {
                let target = std::fs::read_link(&path).map_err(|source| StoreError::Io {
                    path: path.clone(),
                    source,
                })?;
                self.put_symlink(&name, &target, meta)?
            } else {
                let content = std::fs::read(&path).map_err(|source| StoreError::Io {
                    path: path.clone(),
                    source,
                })?;
                self.put(&name, &content, meta)?
            };
            let object_id = store_dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(IngestedEntry {
                rel_path,
                object_id,
                store_dir,
                meta,
            });
        }
        Ok(out)
    }
}

/// 读取文件的语义元数据(unix:真实 mode + symlink 判断)。
///
/// 非 unix 平台无法表达这些语义,返回 [`StoreError::Unsupported`]。
pub fn read_meta(path: impl AsRef<Path>) -> Result<FileMeta> {
    let path = path.as_ref();
    let meta = std::fs::symlink_metadata(path).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let is_symlink = meta.file_type().is_symlink();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 只取权限语义(低 12 位:rwx + setuid/setgid/sticky),屏蔽文件类型位(S_IFREG 等)。
        // 否则 read_meta 读回的 mode 含 0o100000,与入库时传入的纯权限 mode 不一致 → 假 HashMismatch。
        Ok(FileMeta {
            mode: meta.permissions().mode() & 0o7777,
            is_symlink,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = is_symlink;
        Err(StoreError::Unsupported("read_meta(mode)"))
    }
}

/// 显式恢复文件权限位(PoC-6 关键步骤)。非 unix 平台为 no-op(NTFS 无此语义)。
fn restore_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, perms).map_err(|source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(()) // Windows 侧无 unix 权限语义,骨架阶段忽略
    }
}

/// 读取符号链接的 target 字节(与 [`Store::put_symlink`] 入库时的内容表示一致),
/// 供 [`Store::get`] 重算 symlink 对象的哈希做加载期校验。
#[cfg(unix)]
fn read_link_bytes(path: &Path) -> Result<Vec<u8>> {
    let target = std::fs::read_link(path).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(target.as_os_str().as_encoded_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        // 用进程内唯一子目录,避免依赖时钟/随机(确定性约束)。
        std::env::temp_dir().join(format!("aevum-store-test-{}", std::process::id()))
    }

    #[test]
    fn hash_includes_mode() {
        // PoC-6:同内容不同权限位 → 不同 hash。
        let content = b"binary-body";
        let h_plain = Store::hash_blob(content, FileMeta { mode: 0o644, is_symlink: false });
        let h_setuid = Store::hash_blob(content, FileMeta { mode: 0o4755, is_symlink: false });
        assert_ne!(h_plain, h_setuid, "setuid 位必须改变 hash");
    }

    #[test]
    fn hash_stable() {
        let content = b"x";
        let m = FileMeta { mode: 0o755, is_symlink: false };
        assert_eq!(Store::hash_blob(content, m), Store::hash_blob(content, m));
    }

    #[test]
    fn put_and_get_roundtrip() {
        let root = tmp().join("roundtrip");
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::open(&root).unwrap();
        let meta = FileMeta { mode: 0o755, is_symlink: false };
        let dir = store.put("rg", b"rg-binary", meta).unwrap();
        let hash = dir.file_name().unwrap().to_string_lossy();
        let hash = hash.split('-').next().unwrap();
        let got = store.get(hash, "rg").unwrap();
        assert_eq!(got, dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    // setuid 往返恢复测试(PoC-6 核心铁律):put 一个 0o4755 文件后,从 store 读回的
    // 真实 mode 必须仍含 setuid 位,否则 sudo 等提权二进制入库后失效。
    // unix 专有语义,在 WSL/真 Linux 跑;NTFS 无此语义,故 cfg(unix) 守卫。
    #[cfg(unix)]
    #[test]
    fn setuid_bit_survives_roundtrip() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp().join("setuid");
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::open(&root).unwrap();
        // 0o4755 = setuid + rwxr-xr-x
        let meta = FileMeta { mode: 0o4755, is_symlink: false };
        let dir = store.put("sudo", b"fake-sudo", meta).unwrap();
        let file = dir.join("sudo");
        let got_mode = std::fs::symlink_metadata(&file).unwrap().permissions().mode();
        // 低 12 位含权限语义;setuid 位(0o4000)必须保留。
        assert_eq!(got_mode & 0o7777, 0o4755, "入库后 setuid 位必须保留(PoC-6)");
        assert_ne!(got_mode & 0o4000, 0, "setuid 位丢失则提权失效");
        let _ = std::fs::remove_dir_all(&root);
    }

    // get 的加载期内容校验是 unix 语义(hash 含真实 mode);非 unix 仅校验存在,
    // 故篡改检测测试用 cfg(unix) 守卫。
    #[cfg(unix)]
    #[test]
    fn get_detects_tampering() {
        // 加载期校验:篡改 store 内文件内容后 get 应返回 HashMismatch。
        let root = tmp().join("tamper");
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::open(&root).unwrap();
        let meta = FileMeta { mode: 0o644, is_symlink: false };
        let dir = store.put("data", b"original", meta).unwrap();
        let hash = dir.file_name().unwrap().to_string_lossy();
        let hash = hash.split('-').next().unwrap().to_string();
        // 正常取出 OK
        assert!(store.get(&hash, "data").is_ok());
        // 篡改内容
        std::fs::write(dir.join("data"), b"tampered!").unwrap();
        match store.get(&hash, "data") {
            Err(StoreError::HashMismatch { expected, .. }) => assert_eq!(expected, hash),
            other => panic!("篡改后应 HashMismatch,实得 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn ingest_dir_plain_files() {
        // ingest_dir 把普通文件内容寻址入库,保留相对布局。
        let root = tmp().join("ingest");
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("src-pkg");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin/tool"), b"tool-body").unwrap();
        std::fs::write(src.join("readme"), b"hello").unwrap();

        let store = Store::open(root.join("store")).unwrap();
        let entries = store.ingest_dir(&src).unwrap();
        assert_eq!(entries.len(), 2);
        // 确定性排序:bin/tool 在 readme 前
        assert_eq!(entries[0].rel_path, PathBuf::from("bin/tool"));
        assert_eq!(entries[1].rel_path, PathBuf::from("readme"));
        // 每个条目都已入库且可校验取出
        for e in &entries {
            let hash = e.object_id.split('-').next().unwrap();
            let name = e.store_dir.join(e.rel_path.file_name().unwrap());
            assert!(name.exists());
            let nm = e.rel_path.file_name().unwrap().to_str().unwrap();
            assert!(store.get(hash, nm).is_ok());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── P1-10:symlink 保留不解引用是 PoC-5 铁律,与 setuid(已测)同级,补正向测试 ──

    #[cfg(unix)]
    #[test]
    fn put_symlink_roundtrip_and_loadtime_verify() {
        let root = tmp().join("symlink-rt");
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::open(&root).unwrap();
        let meta = FileMeta { mode: 0o777, is_symlink: true };
        // symlink 的"内容"= target 路径字节(指向 store 外的库版本名,典型 soname 链接)。
        let target = Path::new("libc.so.6");
        let dir = store.put_symlink("libc.so", target, meta).unwrap();
        let link = dir.join("libc.so");
        // 确实是 symlink、未解引用、目标保留。
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        // 加载期校验:get() 对 symlink 走 link-target 字节哈希,往返成功。
        let hash = dir.file_name().unwrap().to_string_lossy();
        let hash = hash.split('-').next().unwrap().to_string();
        assert!(store.get(&hash, "libc.so").is_ok(), "symlink 对象应能加载期校验通过");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn get_detects_symlink_target_tamper() {
        // 篡改 symlink 指向(重指到别的 target)→ 哈希变化 → HashMismatch。
        let root = tmp().join("symlink-tamper");
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::open(&root).unwrap();
        let meta = FileMeta { mode: 0o777, is_symlink: true };
        let dir = store.put_symlink("l", Path::new("good-target"), meta).unwrap();
        let hash = dir.file_name().unwrap().to_string_lossy();
        let hash = hash.split('-').next().unwrap().to_string();
        assert!(store.get(&hash, "l").is_ok());
        // 重指:删旧链接,建指向 evil-target 的新链接(同名)。
        let link = dir.join("l");
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("evil-target", &link).unwrap();
        match store.get(&hash, "l") {
            Err(StoreError::HashMismatch { expected, .. }) => assert_eq!(expected, hash),
            other => panic!("篡改 symlink target 后应 HashMismatch,实得 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn ingest_dir_preserves_symlink_without_deref() {
        // PoC-5:ingest_dir 遇 symlink 必须保留为 symlink(is_symlink=true),不跟进目标。
        let root = tmp().join("ingest-symlink");
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("pkg");
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::write(src.join("lib/libfoo.so.1.2.3"), b"real-lib-body").unwrap();
        // soname 链接:libfoo.so.1 → libfoo.so.1.2.3(动态链接器按 soname 查找的典型布局)。
        std::os::unix::fs::symlink("libfoo.so.1.2.3", src.join("lib/libfoo.so.1")).unwrap();

        let store = Store::open(root.join("store")).unwrap();
        let entries = store.ingest_dir(&src).unwrap();
        let link_entry = entries
            .iter()
            .find(|e| e.rel_path == PathBuf::from("lib/libfoo.so.1"))
            .expect("应有 libfoo.so.1 条目");
        assert!(link_entry.meta.is_symlink, "soname 链接必须保留为 symlink(未解引用)");
        // store 里它确实是个 symlink,而非被解引用复制成普通文件。
        let stored = link_entry.store_dir.join("libfoo.so.1");
        assert!(
            std::fs::symlink_metadata(&stored).unwrap().file_type().is_symlink(),
            "store 内对象应仍是 symlink"
        );
        assert_eq!(std::fs::read_link(&stored).unwrap(), PathBuf::from("libfoo.so.1.2.3"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
