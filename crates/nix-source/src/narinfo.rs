//! narinfo 解析:Nix binary cache 的包元数据格式。
//!
//! 格式:plain text,每行 `Key: Value`。
//! 关键字段:StorePath, URL, Compression, FileHash, FileSize, NarHash, NarSize, References, Sig。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NarInfoError {
    #[error("narinfo 缺少必要字段: {0}")]
    MissingField(String),
    #[error("narinfo 格式错误: {0}")]
    Format(String),
}

/// Nix binary cache 的包元数据。
#[derive(Debug, Clone, Default)]
pub struct NarInfo {
    /// 完整 store path:`/nix/store/<hash>-<name>`
    pub store_path: String,
    /// NAR 文件相对 URL:`nar/<hash>.nar.xz`
    pub url: String,
    /// 压缩方式(通常 `xz`)
    pub compression: String,
    /// 压缩文件的 hash
    pub file_hash: String,
    /// 压缩文件大小(字节)
    pub file_size: u64,
    /// 解压后 NAR 的 hash
    pub nar_hash: String,
    /// 解压后 NAR 大小
    pub nar_size: u64,
    /// 依赖的 store path 名列表(不含 `/nix/store/` 前缀,空格分隔)
    pub references: Vec<String>,
    /// 签名
    pub sig: String,
}

impl NarInfo {
    /// 从 narinfo 文本解析。
    pub fn parse(text: &str) -> Result<Self, NarInfoError> {
        let mut info = NarInfo::default();
        for line in text.lines() {
            let Some(colon) = line.find(':') else { continue };
            let key = line[..colon].trim();
            let value = line[colon + 1..].trim();
            match key {
                "StorePath" => info.store_path = value.to_string(),
                "URL" => info.url = value.to_string(),
                "Compression" => info.compression = value.to_string(),
                "FileHash" => info.file_hash = value.to_string(),
                "FileSize" => info.file_size = value.parse().unwrap_or(0),
                "NarHash" => info.nar_hash = value.to_string(),
                "NarSize" => info.nar_size = value.parse().unwrap_or(0),
                "References" => {
                    info.references = value
                        .split_whitespace()
                        .map(|s| s.to_string())
                        .collect();
                }
                "Sig" => info.sig = value.to_string(),
                _ => {} // 忽略未知字段(Deriver 等)
            }
        }
        if info.store_path.is_empty() {
            return Err(NarInfoError::MissingField("StorePath".into()));
        }
        if info.url.is_empty() {
            return Err(NarInfoError::MissingField("URL".into()));
        }
        Ok(info)
    }

    /// 提取 store path 的 32 字符 hash 部分。
    /// `/nix/store/f4y36sn7m173qvdija8a1p6v81py66ns-niri-26.04` → `f4y36sn7m173qvdija8a1p6v81py66ns`
    pub fn hash(&self) -> &str {
        let name = self.store_path.strip_prefix("/nix/store/").unwrap_or(&self.store_path);
        name.split('-').next().unwrap_or(name)
    }

    /// 提取包名(去掉 hash 前缀)。
    /// `/nix/store/f4y36sn7m173qvdija8a1p6v81py66ns-niri-26.04` → `niri-26.04`
    pub fn name(&self) -> &str {
        let name = self.store_path.strip_prefix("/nix/store/").unwrap_or(&self.store_path);
        match name.find('-') {
            Some(pos) => &name[pos + 1..],
            None => name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"StorePath: /nix/store/f4y36sn7m173qvdija8a1p6v81py66ns-niri-26.04
URL: nar/12ia6izzr2f0nppmpf9qldpi1mw9wqby5lyxx4idvjxndk4vfpgw.nar.xz
Compression: xz
FileHash: sha256:12ia6izzr2f0nppmpf9qldpi1mw9wqby5lyxx4idvjxndk4vfpgw
FileSize: 7357612
NarHash: sha256:023ch6pafmg4f6sjwy6kwispy28fhz3qxrwbqjzizwbp37c0cbiq
NarSize: 39198968
References: 57iz36553175g3178pvxjij8z5rcsd4n-glibc-2.42-61 chqq8mpmpyfi9kgsngya71akv5xicn03-gcc-15.2.0-lib f4y36sn7m173qvdija8a1p6v81py66ns-niri-26.04
Deriver: y2ypwz3yc1xdclaars9hmndgdsc91rsk-niri-26.04.drv
Sig: cache.nixos.org-1:A280XsxdSgHu2NO8KKju5Wvf7a1JgtH0Yp5c6Btqc4Rnvd/lA1Dpi6MEwTPTziIlxyN2HGKw9KVFzAYo8jw5Dg=="#;

    #[test]
    fn parse_narinfo() {
        let info = NarInfo::parse(SAMPLE).unwrap();
        assert_eq!(info.store_path, "/nix/store/f4y36sn7m173qvdija8a1p6v81py66ns-niri-26.04");
        assert_eq!(info.url, "nar/12ia6izzr2f0nppmpf9qldpi1mw9wqby5lyxx4idvjxndk4vfpgw.nar.xz");
        assert_eq!(info.compression, "xz");
        assert_eq!(info.file_size, 7357612);
        assert_eq!(info.nar_size, 39198968);
        assert_eq!(info.references.len(), 3);
        assert!(info.references.iter().any(|r| r.contains("glibc")));
    }

    #[test]
    fn hash_and_name() {
        let info = NarInfo::parse(SAMPLE).unwrap();
        assert_eq!(info.hash(), "f4y36sn7m173qvdija8a1p6v81py66ns");
        assert_eq!(info.name(), "niri-26.04");
    }

    #[test]
    fn missing_store_path_errors() {
        let bad = "URL: nar/foo.nar.xz\n";
        assert!(matches!(NarInfo::parse(bad), Err(NarInfoError::MissingField(_))));
    }
}
