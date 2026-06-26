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

    /// 安全提取落盘用的 ref 名(`<hash>-<name>`),拒绝路径穿越。
    ///
    /// **安全关键**:`store_path` 来自下载的 narinfo(镜像/服务器控制),绝不可信。
    /// 天真地 `strip_prefix("/nix/store/").unwrap_or(&store_path)` 再 `store_dir.join(..)`
    /// 会被恶意 narinfo 利用:`StorePath: /etc/cron.d/evil`(无前缀)在 Unix 下经
    /// `Path::join` 丢弃 base 直接写绝对路径;`StorePath: /nix/store/../../etc/x` 经 `..` 上跳。
    /// 二者都能把包内容(含可执行位/setuid/symlink)写到任意可写路径 → 任意文件写。
    ///
    /// 校验:必须 `/nix/store/` 前缀;剩余段不得含路径分隔符/空字节、不得为 `.`/`..`、
    /// 不得为空;且须形如 `<32 字符 hash>-<name>`。通过后 ref 名保证是 store_dir 的直接子项。
    pub fn validated_ref_name(&self) -> Result<&str, NarInfoError> {
        let rest = self.store_path.strip_prefix("/nix/store/").ok_or_else(|| {
            NarInfoError::Format(format!("StorePath 不以 /nix/store/ 开头(拒绝): {}", self.store_path))
        })?;
        if rest.is_empty()
            || rest == "."
            || rest == ".."
            || rest.contains('/')
            || rest.contains('\\')
            || rest.contains('\0')
        {
            return Err(NarInfoError::Format(format!(
                "StorePath 含非法 ref 名(疑路径穿越,拒绝): {rest:?}"
            )));
        }
        // 形如 <hash>-<name>:至少 34 字符,第 33 位(下标 32)是 '-'。
        if rest.len() < 34 || rest.as_bytes()[32] != b'-' {
            return Err(NarInfoError::Format(format!(
                "StorePath ref 名格式非法(应为 <32 字符 hash>-<name>): {rest:?}"
            )));
        }
        Ok(rest)
    }

    /// 校验解压后 NAR 的内容哈希是否匹配 narinfo 的 `NarHash`。
    ///
    /// `nar_sha256` 是调用方对**完整 NAR 字节流**算出的 SHA256 摘要(32 字节)。
    /// narinfo 的 `NarHash` 形如 `sha256:<nixbase32>`(Nix 自定义 base32,52 字符)。
    /// 把摘要编成 nixbase32 与之逐字符比对。
    ///
    /// **安全意义**:这是 NAR 下载的完整性闸门——传输损坏 / 中间人篡改 / 镜像投毒
    /// 都会导致摘要不符而被拒。没有它,`curl | xz | unpack` 等于无条件信任镜像字节。
    /// 仅支持 sha256(Nix 现行缓存唯一在用);其它算法返回错误而非静默放行。
    pub fn verify_nar_hash(&self, nar_sha256: &[u8]) -> Result<(), NarInfoError> {
        if self.nar_hash.is_empty() {
            return Err(NarInfoError::MissingField("NarHash".into()));
        }
        let (algo, want) = self
            .nar_hash
            .split_once(':')
            .ok_or_else(|| NarInfoError::Format(format!("NarHash 缺算法前缀: {}", self.nar_hash)))?;
        if algo != "sha256" {
            return Err(NarInfoError::Format(format!(
                "NarHash 算法不支持(仅 sha256): {algo}"
            )));
        }
        let got = crate::nix_base32::encode(nar_sha256);
        if got != want {
            return Err(NarInfoError::Format(format!(
                "NAR 内容哈希不匹配(完整性校验失败):期望 sha256:{want},实得 sha256:{got}"
            )));
        }
        Ok(())
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

    #[test]
    fn verify_nar_hash_accepts_matching() {
        use sha2::{Digest, Sha256};
        // 任取一段字节算 sha256 → 编成 nixbase32 → 塞进 NarHash → verify 应通过。
        let payload = b"the-nar-bytes-whatever";
        let digest: [u8; 32] = Sha256::digest(payload).into();
        let nixb32 = crate::nix_base32::encode(&digest);
        let info = NarInfo {
            store_path: "/nix/store/f4y36sn7m173qvdija8a1p6v81py66ns-x-1".into(),
            url: "nar/x.nar.xz".into(),
            nar_hash: format!("sha256:{nixb32}"),
            ..Default::default()
        };
        assert!(info.verify_nar_hash(&digest).is_ok());
    }

    #[test]
    fn verify_nar_hash_rejects_tamper() {
        use sha2::{Digest, Sha256};
        let good: [u8; 32] = Sha256::digest(b"real").into();
        let info = NarInfo {
            nar_hash: format!("sha256:{}", crate::nix_base32::encode(&good)),
            ..Default::default()
        };
        // 篡改后的字节摘要不同 → 必须拒绝。
        let tampered: [u8; 32] = Sha256::digest(b"evil").into();
        assert!(info.verify_nar_hash(&tampered).is_err());
    }

    #[test]
    fn verify_nar_hash_rejects_missing_and_bad_algo() {
        let no_hash = NarInfo::default();
        assert!(no_hash.verify_nar_hash(&[0u8; 32]).is_err());

        let md5 = NarInfo { nar_hash: "md5:deadbeef".into(), ..Default::default() };
        assert!(md5.verify_nar_hash(&[0u8; 32]).is_err());

        let no_prefix = NarInfo { nar_hash: "justhexnocolon".into(), ..Default::default() };
        assert!(no_prefix.verify_nar_hash(&[0u8; 32]).is_err());
    }

    #[test]
    fn validated_ref_name_accepts_legit() {
        let info = NarInfo::parse(SAMPLE).unwrap();
        assert_eq!(
            info.validated_ref_name().unwrap(),
            "f4y36sn7m173qvdija8a1p6v81py66ns-niri-26.04"
        );
    }

    #[test]
    fn validated_ref_name_rejects_path_traversal() {
        // 构造恶意 narinfo:各种路径穿越/绝对路径逃逸,必须全被拒。
        let attacks = [
            "/etc/cron.d/evil",                       // 无 /nix/store/ 前缀 → 绝对路径逃逸
            "/nix/store/../../etc/passwd",            // .. 上跳
            "/nix/store/",                            // 空 ref
            "/nix/store/.",                           // 当前目录
            "/nix/store/..",                          // 父目录
            "/nix/store/sub/dir-name",                // 含分隔符 → 非直接子项
            "relative-path",                          // 完全无前缀
        ];
        for sp in attacks {
            let info = NarInfo {
                store_path: sp.to_string(),
                url: "nar/x.nar.xz".into(),
                ..Default::default()
            };
            assert!(
                info.validated_ref_name().is_err(),
                "路径穿越未被拒绝: {sp:?}"
            );
        }
    }

    #[test]
    fn validated_ref_name_rejects_bad_shape() {
        // 有前缀但不形如 <32 hash>-<name>。
        for sp in ["/nix/store/short", "/nix/store/f4y36sn7m173qvdija8a1p6v81py66nsXniri"] {
            let info = NarInfo { store_path: sp.to_string(), url: "u".into(), ..Default::default() };
            assert!(info.validated_ref_name().is_err(), "坏格式未被拒: {sp:?}");
        }
    }
}
