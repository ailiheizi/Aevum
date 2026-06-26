# Nix Binary Cache 包源设计(crates/nix-source)

> 状态:待实现 · 日期:2026-06-24
> 前置验证:本会话已用 Python 脚本 + curl 验证全链路可行(从 USTC 镜像递归拉 239 包,niri 26.04 成功运行)

---

## 0. 目标

让 Aevum 能消费 Nix binary cache 作为第二包源(除 Debian .deb 外),从镜像下载预编译包并管理。用户视角:

```bash
aevum maintain --source nix --packages niri --mirror https://mirrors.ustc.edu.cn/nix-channels/store
```

等价于:从 Nix cache 递归拉 niri 及其全部传递依赖 → 解包到 Aevum store → 建世代 → 激活。

---

## 1. 协议(已验证)

Nix binary cache 是纯 HTTP 的简单协议:

```
GET /nix-cache-info          → StoreDir: /nix/store\nWantMassQuery: 1\n...
GET /<hash>.narinfo          → StorePath/URL/FileHash/NarHash/References/Sig
GET /nar/<hash>.nar.xz       → xz 压缩的 NAR 归档
```

### narinfo 格式示例(niri):
```
StorePath: /nix/store/f4y36sn7m173qvdija8a1p6v81py66ns-niri-26.04
URL: nar/12ia6izzr2f0nppmpf9qldpi1mw9wqby5lyxx4idvjxndk4vfpgw.nar.xz
Compression: xz
FileHash: sha256:12ia6izzr2f0nppmpf9qldpi1mw9wqby5lyxx4idvjxndk4vfpgw
FileSize: 7357612
NarHash: sha256:023ch6pafmg4f6sjwy6kwispy28fhz3qxrwbqjzizwbp37c0cbiq
NarSize: 39198968
References: 57iz36553175g3178pvxjij8z5rcsd4n-glibc-2.42-61 chqq8mpmpyfi9kgsngya71akv5xicn03-gcc-15.2.0-lib ...
Sig: cache.nixos.org-1:A280Xs...
```

### NAR 格式(二进制归档)
```
"nix-archive-1" → 递归节点:
  "(" "type" "regular"    → ["executable" ""] "contents" <u64:size> <data> <padding> ")"
  "(" "type" "directory"  → ("entry" "(" "name" <str> "node" <递归> ")")* ")"
  "(" "type" "symlink"    → "target" <str> ")"
```
字符串:u64 长度 + 数据 + 对齐到 8 字节。

---

## 2. 设计

### 新 crate: `crates/nix-source`

**依赖**:thiserror(错误)。不依赖 reqwest/hyper——用系统 `curl` 下载(与现有 Debian 源一致,零网络依赖)。xz 解压用 `xz` 命令(系统已有)或考虑引入 `lzma-rs`(纯 Rust)。

#### 2.1 `nar.rs` — NAR 解包器

```rust
/// 从 Reader 解包 NAR 到目标目录(递归创建文件/目录/symlink)。
pub fn unpack_nar(reader: &mut impl Read, dest: &Path) -> Result<usize, NarError>;
```

实现直译 Nix 的 `archive.cc` parse():
- read_string: u64 len + data + align(8)
- 递归 parse: type=regular(executable+contents) / directory(entry loop) / symlink(target)
- 安全限制:max depth 64,max name 255,max target 4095(同 Nix)

**借鉴自**: `nix-master/src/libutil/archive.cc:178` parse() 函数。

#### 2.2 `narinfo.rs` — narinfo 解析

```rust
pub struct NarInfo {
    pub store_path: String,      // /nix/store/<hash>-<name>
    pub url: String,             // nar/<hash>.nar.xz
    pub compression: String,     // xz
    pub file_hash: String,       // sha256:...
    pub file_size: u64,
    pub nar_hash: String,
    pub nar_size: u64,
    pub references: Vec<String>, // 依赖的 store path 名(不含 /nix/store/ 前缀)
    pub sig: String,
}

impl NarInfo {
    pub fn parse(text: &str) -> Result<Self, NarInfoError>;
    /// 提取 32 字符 hash(store path 的 hash 部分)
    pub fn hash(&self) -> &str;
}
```

**借鉴自**: `nix-master/src/libstore/nar-info.cc` 的 key:value 行解析。

#### 2.3 `cache_client.rs` — binary cache 客户端

```rust
pub struct NixCacheClient {
    pub mirror: String,   // e.g. "https://mirrors.ustc.edu.cn/nix-channels/store"
    pub store_dir: PathBuf,  // 目标:/nix/store 或 Aevum 的等价路径
}

impl NixCacheClient {
    /// 拉单个包:下载 narinfo → 下载 NAR → 解压 → unpack 到 store_dir
    pub fn fetch_one(&self, hash: &str) -> Result<NarInfo, CacheError>;

    /// 递归拉包及其全部传递依赖(BFS/DFS,visited 去重)
    pub fn fetch_closure(&self, root_hash: &str) -> Result<Vec<NarInfo>, CacheError>;

    /// 从 store-paths.xz 查包名对应的 hash
    pub fn resolve_package(&self, name: &str) -> Result<String, CacheError>;
}
```

下载实现:调系统 `curl -sL <url>`,管道到 `xz -d`,再管道到 `unpack_nar`。与现有 Debian 源的下载方式一致(fork curl 子进程)。

**借鉴自**: `nix-master/src/libstore/binary-cache-store.cc` queryPathInfoUncached + narFromPath。

---

## 3. 与 Aevum 集成

### 3.1 Store 布局选择

**方案 A**(推荐):直接用 `/nix/store/<hash>-<name>` 布局
- 优点:Nix 包的 RUNPATH/interpreter 全部 hardcode 这个路径,不需要 patchelf
- 缺点:需要 `/nix/store` 目录(root 创建一次即可)

**方案 B**:放进 Aevum 自己的 store + patchelf 改路径
- 优点:不依赖 `/nix/store`
- 缺点:patchelf 每个 ELF 很慢,且 RUNPATH 里的交叉引用极多

**结论**:方案 A。`/nix/store` 只是一个目录名,不需要安装 Nix。Aevum 自己管理其内容。

### 3.2 CLI 接入

```
aevum maintain --source nix --packages niri --gen 10 --mirror <url>
```

或 TS 配置:
```typescript
export default defineSystem(() => ({
  nixPackages: ["niri", "foot"],  // 从 Nix cache 拉
  uses: ["busybox-static"],       // 从 Debian 拉
}));
```

### 3.3 Profile/PATH

Nix 包的二进制在 `/nix/store/<hash>-<name>/bin/`。profile/bin 建 symlink 指向这里。

---

## 4. 实现顺序

1. `nar.rs`(NAR 解包器,~100 行 Rust,有 Python 原型可直译)
2. `narinfo.rs`(narinfo 解析,~50 行)
3. `cache_client.rs`(curl + xz + unpack 管道,递归拉依赖,~150 行)
4. CLI 集成(`--source nix`)
5. TS 配置集成(`nixPackages` 字段)

---

## 5. 已验证的事实(本会话实验)

- USTC 镜像 `mirrors.ustc.edu.cn/nix-channels/store` 可达,协议正确
- `store-paths.xz` 可下载解压搜索包名→hash
- narinfo 解析 + NAR 下载 + xz 解压 + Python unpack 全链路跑通
- 递归拉 niri 的 239 个传递依赖全部成功
- `/nix/store` 布局下 niri 26.04 `--version` 成功执行
- niri 需要 EGL/DRM(WSLg 不完整支持),但 weston 能在 WSLg 跑(已验证)

---

## 6. 参考代码位置(已克隆到 refs/nix-master/)

| 文件 | 内容 |
|------|------|
| `src/libutil/archive.cc:178` | NAR parse()(反序列化核心) |
| `src/libutil/archive.cc:147` | parseContents()(读文件内容+padding) |
| `src/libstore/nar-info.cc:10` | NarInfo 构造(narinfo 文本解析) |
| `src/libstore/binary-cache-store.cc:587` | queryPathInfoUncached(拉 narinfo) |
| `src/libstore/binary-cache-store.cc` | narFromPath(拉 NAR) |
| `src/libstore/profiles.cc` | profile 世代管理(symlink farm) |
