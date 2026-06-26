//! Nix binary cache 包源:让 Aevum 消费 Nix 预编译包(见 `docs/design/nix-source.md`)。
//!
//! 协议:纯 HTTP,从镜像(如 `mirrors.ustc.edu.cn/nix-channels/store`)拉取:
//! 1. `<hash>.narinfo` → 包元数据(StorePath/URL/References)
//! 2. `nar/<hash>.nar.xz` → NAR 归档(xz 压缩的文件树)
//!
//! 本 crate 提供:
//! - [`nar::unpack`]:NAR 格式解包(二进制归档 → 文件树)
//! - [`narinfo::NarInfo`]:narinfo 文本解析
//! - [`cache::NixCacheClient`]:HTTP 客户端(递归拉依赖闭包)

pub mod nar;
pub mod narinfo;
pub mod cache;
pub mod nix_base32;

pub use cache::NixCacheClient;
pub use narinfo::NarInfo;
