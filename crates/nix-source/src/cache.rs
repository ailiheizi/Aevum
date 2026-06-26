//! Nix binary cache HTTP 客户端:从镜像递归拉包及其依赖闭包。
//!
//! 下载实现:调系统 `curl -sL` + `xz -d` 管道(与 Aevum 现有 Debian 源一致,零网络依赖)。
//! 递归拉依赖:BFS,visited 去重。

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::nar;
use crate::narinfo::NarInfo;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("narinfo 拉取失败({hash}): {reason}")]
    NarInfoFetch { hash: String, reason: String },
    #[error("narinfo 解析失败: {0}")]
    NarInfoParse(#[from] crate::narinfo::NarInfoError),
    #[error("NAR 下载/解包失败({url}): {reason}")]
    NarFetch { url: String, reason: String },
    #[error("NAR 解包失败: {0}")]
    NarUnpack(#[from] nar::NarError),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("包未找到: {0}")]
    NotFound(String),
}

/// Nix binary cache 客户端。
pub struct NixCacheClient {
    /// 镜像 URL(如 `https://mirrors.ustc.edu.cn/nix-channels/store`)
    pub mirror: String,
    /// 目标 store 目录(默认 `/nix/store`)
    pub store_dir: PathBuf,
}

impl NixCacheClient {
    pub fn new(mirror: impl Into<String>, store_dir: impl Into<PathBuf>) -> Self {
        Self {
            mirror: mirror.into(),
            store_dir: store_dir.into(),
        }
    }

    /// 拉取单个包的 narinfo。
    pub fn fetch_narinfo(&self, hash: &str) -> Result<NarInfo, CacheError> {
        let url = format!("{}/{}.narinfo", self.mirror, hash);
        let output = Command::new("curl")
            .args(["-sL", "--fail", &url])
            .output()
            .map_err(|e| CacheError::NarInfoFetch {
                hash: hash.to_string(),
                reason: format!("curl 执行失败: {e}"),
            })?;
        if !output.status.success() {
            return Err(CacheError::NarInfoFetch {
                hash: hash.to_string(),
                reason: format!("HTTP 失败(status {})", output.status),
            });
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(NarInfo::parse(&text)?)
    }

    /// 拉取并解包单个 NAR 到 store_dir。
    ///
    /// 管道:`curl -sL <nar_url> | xz -d` → NAR reader → unpack 到 `store_dir/<ref>`。
    /// 如果目标目录已存在则跳过(幂等)。
    pub fn fetch_and_unpack(&self, info: &NarInfo) -> Result<usize, CacheError> {
        // 目标路径:store_dir/<hash>-<name>
        let ref_name = info.store_path.strip_prefix("/nix/store/").unwrap_or(&info.store_path);
        let dest = self.store_dir.join(ref_name);
        if dest.exists() {
            return Ok(0); // 已存在,跳过
        }

        let nar_url = format!("{}/{}", self.mirror, info.url);

        // curl | xz -d → pipe
        let mut curl = Command::new("curl")
            .args(["-sL", "--fail", &nar_url])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| CacheError::NarFetch {
                url: nar_url.clone(),
                reason: format!("curl spawn: {e}"),
            })?;

        let curl_stdout = curl.stdout.take().unwrap();

        let mut xz = Command::new("xz")
            .args(["-d"])
            .stdin(curl_stdout)
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| CacheError::NarFetch {
                url: nar_url.clone(),
                reason: format!("xz spawn: {e}"),
            })?;

        let mut nar_stream = xz.stdout.take().unwrap();
        let count = nar::unpack(&mut nar_stream, &dest)?;

        // 等子进程结束
        let _ = curl.wait();
        let xz_status = xz.wait()?;
        if !xz_status.success() {
            // 清理不完整的解包
            let _ = std::fs::remove_dir_all(&dest);
            return Err(CacheError::NarFetch {
                url: nar_url,
                reason: format!("xz 解压失败(status {})", xz_status),
            });
        }

        Ok(count)
    }

    /// 拉取单个包:narinfo → 下载 NAR → 解包。返回 narinfo。
    pub fn fetch_one(&self, hash: &str) -> Result<NarInfo, CacheError> {
        let info = self.fetch_narinfo(hash)?;
        self.fetch_and_unpack(&info)?;
        Ok(info)
    }

    /// 递归拉取包及其全部传递依赖(BFS,visited 去重)。
    ///
    /// 返回所有拉取的 NarInfo(按拉取顺序)。已在 store 中的包仍返回其 narinfo(用于元数据),
    /// 但不重复下载。
    pub fn fetch_closure(&self, root_hash: &str) -> Result<Vec<NarInfo>, CacheError> {
        let mut queue: VecDeque<String> = VecDeque::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        let mut results: Vec<NarInfo> = Vec::new();

        queue.push_back(root_hash.to_string());

        while let Some(hash) = queue.pop_front() {
            if visited.contains(&hash) {
                continue;
            }
            visited.insert(hash.clone());

            let info = self.fetch_narinfo(&hash)?;
            self.fetch_and_unpack(&info)?;

            // 把依赖加入队列
            for dep_ref in &info.references {
                let dep_hash = dep_ref.split('-').next().unwrap_or(dep_ref);
                if !visited.contains(dep_hash) {
                    queue.push_back(dep_hash.to_string());
                }
            }

            results.push(info);
        }

        Ok(results)
    }

    /// 从 store-paths.xz 搜索包名对应的 store path hash。
    ///
    /// 需要 channel URL(如 `https://mirrors.ustc.edu.cn/nix-channels/nixpkgs-unstable`)。
    /// 下载 `store-paths.xz` → 解压 → grep 包名 → 提取 hash。
    /// 优先选不带 `-doc`/`-dev`/`-man`/`-info` 后缀的精确匹配。
    pub fn resolve_package(channel_url: &str, name: &str) -> Result<String, CacheError> {
        let url = format!("{}/store-paths.xz", channel_url);
        let output = Command::new("sh")
            .args(["-c", &format!("curl -sL '{}' | xz -d | grep -F -- '-{}'", url, name)])
            .output()
            .map_err(|e| CacheError::NotFound(format!("resolve 执行失败: {e}")))?;

        let text = String::from_utf8_lossy(&output.stdout);
        let mut candidates: Vec<(String, String)> = Vec::new(); // (hash, full_pkg_part)

        for line in text.lines() {
            let store_name = line.strip_prefix("/nix/store/").unwrap_or(line).trim();
            if store_name.is_empty() {
                continue;
            }
            // hash 是前 32 字符(Nix base32),后面是 `-<name>[-<version>]`
            if store_name.len() < 34 || store_name.as_bytes()[32] != b'-' {
                continue;
            }
            let hash = &store_name[..32];
            let pkg_part = &store_name[33..];

            // 精确匹配:pkg_part == name 或 pkg_part == name-<version>
            if pkg_part == name
                || pkg_part.starts_with(&format!("{name}-"))
                    && !pkg_part.ends_with("-doc")
                    && !pkg_part.ends_with("-dev")
                    && !pkg_part.ends_with("-man")
                    && !pkg_part.ends_with("-info")
                    && !pkg_part.ends_with("-lib")
            {
                candidates.push((hash.to_string(), pkg_part.to_string()));
            }
        }

        // 优先选精确匹配(pkg_part 最短/最精确)
        candidates.sort_by_key(|(_, pkg)| pkg.len());

        if let Some((hash, pkg)) = candidates.first() {
            eprintln!("  resolve: {name} → {hash}-{pkg}");
            return Ok(hash.to_string());
        }
        Err(CacheError::NotFound(format!("在 store-paths 中未找到包 '{name}'")))
    }
}

/// 获取 store 目录中已有的包 hash 集合(用于跳过已下载的)。
pub fn existing_store_hashes(store_dir: &Path) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(store_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if let Some(hash) = name.split('-').next() {
                    set.insert(hash.to_string());
                }
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_store_hashes_works() {
        let dir = std::env::temp_dir().join(format!("nix-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("abc123-hello")).unwrap();
        std::fs::create_dir_all(dir.join("def456-world")).unwrap();
        let hashes = existing_store_hashes(&dir);
        assert!(hashes.contains("abc123"));
        assert!(hashes.contains("def456"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
