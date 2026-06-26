# Store —— 内容寻址的不可变存储

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:[`02-generation.md`](02-generation.md)、[`03-closure.md`](03-closure.md)、GC [`../../ai/03-garbage-collection.md`](../../ai/03-garbage-collection.md)

---

## 0. 设计哲学

> **每个包按它的内容寻址,一旦写入永不修改。"改"等于"产生一个新地址"。**

这条原则是 Aevum 一切可复现性、原子性、多版本并存、安全回滚的物理基础。它直接借鉴 NixOS 的 `/nix/store`,但把命名与校验规则收紧、并明确多版本并存与去重语义。

---

## 1. 是什么

Store 是一个内容寻址(content-addressed)的目录,所有"制品(artifact)"按其内容的哈希存放:

```text
<aevum_root>/store/
├── sha256-3a7bd3e2.../        python@3.11.8
│   ├── meta.toml              包元数据（名称、版本、来源、provides/requires）
│   ├── closure.toml           该包自身声明的直接依赖
│   └── content/               实际文件树（bin / lib / share ...）
├── sha256-9f86d081.../        python@3.12.2     ← 不同版本，不同 hash，并存
├── sha256-2c624232.../        openssl@3.2.1
└── .links/                    去重用的硬链接索引（见 §5）
```

- 目录名 = `sha256-` + 该包内容树的规范化哈希。
- 制品可以是预编译二进制、共享库、配置产物 —— Aevum 不强制从源码构建(与 NixOS 不同,见 §6)。

---

## 2. 命名规则

```text
sha256-<hex>
```

- 哈希算法:SHA-256(`meta.toml` 中保留 `algo` 字段,便于未来迁移到更强算法)。
- 哈希输入:对 `content/` 目录做**规范化序列化**(固定排序、清零 mtime)后取摘要,保证同样的内容在任何机器上得到同样的 hash。
- **语义权限位必须保留进哈希输入,不可一律归一**(PoC-6 发现):可执行位、**setuid/setgid/sticky** 是包语义的一部分。`/usr/bin/sudo` 的 `0o4755` 与去掉 setuid 的 `0o0755` 是两个不同的东西,必须产生不同 hash,否则 setuid 包入 store 后提权失效、或与非特权版混淆。只归一无语义的噪声(如 mtime),不归一权限语义。
- **入库与取出必须显式恢复完整权限位**(含 setuid/setgid):内容寻址只复制"内容字节",权限位是带外 inode 元数据,天真的 read→write 复制会丢失(PoC-6 实测:sudo 复制后 `0o4755`→`0o644`,提权失效)。store 须记录并恢复。
- **符号链接是内容的一部分,规范化时保留为符号链接、不解引用成副本**(PoC-5 发现:复杂包大量用符号链接做版本别名/多命令共享一个二进制,如 imagemagick 的 137 个工具软链到 `magick`、python 的 `python→python3→python3.14`;解引用会爆量复制并破坏包预期布局)。
- 目录名可用哈希前缀缩短显示,但完整 hash 始终记录在 `meta.toml`。

> **规范化是可复现的命脉**:两台机器装同一个包,内容字节一致 → hash 一致 → 闭包一致 → 世代一致。

---

## 3. 不可变性

1. 写入流程:先写到临时目录 → 校验 → 规范化 → 原子 `rename` 到最终 hash 目录 → 设为只读。
2. 写入后目录权限设为只读(尽力而为;真正的强制在加载期的哈希校验)。
3. 任何"升级/修补"都不会原地改 —— 它产生一个**新 hash 目录**,旧的原样保留。

这就是为什么回滚是安全的:回滚目标世代引用的所有 hash 都还在,内容和当初逐字节一致。

---

## 4. 完整性校验

加载任意包进入某个世代时:

```text
1. 读取目录名中的 hash
2. 重算 content/ 的规范化哈希
3. 比对：
   ├─ 一致 → 放行
   └─ 不一致 → 报错（磁盘损坏 / 被篡改）→ 标记失效 → 从来源重新拉取
```

校验失败永远不会污染世代:要么拿到正确内容,要么这个世代 verify 不通过。

---

## 5. 多版本并存与去重

### 5.1 多版本并存

同一个包的多个版本是**不同的 hash 目录**,天然并存,互不冲突:

```text
store/sha256-aaa.../   →  openssl@3.0.13   （旧软件还在用）
store/sha256-bbb.../   →  openssl@3.2.1    （新软件用）
```

这是"实在解不了就保留两份"(见 [`../../ai/02-repair-and-keep-two.md`](../../ai/02-repair-and-keep-two.md))的存储层基础:当两个软件对同一依赖版本要求互斥、无法消解时,Aevum 不强行二选一,而是让两个版本在 store 中共存,各自的闭包引用各自需要的 hash。

### 5.2 去重

- 同一个 hash 被多个世代/多个包引用时,store 里只有一份。
- `content/` 内部相同的文件(跨包重复的库)可经 `.links/` 做硬链接级去重(可选优化,语义上对上层透明)。

---

## 6. 与 NixOS store 的关键差异

| 维度 | NixOS `/nix/store` | Aevum store |
|---|---|---|
| 寻址 | 输入寻址(derivation 的输入哈希)为主 | **内容寻址**(产物内容哈希)为主,更直观、更易跨机校验 |
| 来源 | 几乎一切从源码经 derivation 构建 | **不强制从源码构建**;预编译二进制是一等公民(回应二进制摩擦痛点) |
| 表达 | 由 Nix 语言 derivation 决定放什么 | 由 Maintainer 求解的 closure 决定;无 DSL |
| 二进制兼容 | 硬编码 `/nix/store` 路径,普通二进制跑不起来 | 配套 System 层的链接器基线让普通二进制可跑(见 [`../runtime/03-binary-compat.md`](../runtime/03-binary-compat.md)) |

> 选内容寻址而非输入寻址的理由,以及不强制源码构建的取舍,记录在 [`../adr/0001-positioning-vs-nixos.md`](../adr/0001-positioning-vs-nixos.md)。

---

## 7. 数据结构(草案)

```toml
# store/sha256-<hash>/meta.toml
schema_version = "1.0"

[package]
name = "python"
version = "3.11.8"
hash = "sha256-3a7bd3e2..."
algo = "sha256"

[source]
kind = "binary"                 # binary / built / config
origin = "aevum-index:python@3.11.8"   # aevum-index = 索引与供给管线的产出,见 04-index-and-supply.md
fetched_at = "2026-06-08T00:00:00Z"

[provides]
# 该包对外提供的能力标识（供 closure 求解匹配 requires）
capabilities = ["python3", "python3.11"]

[requires]
# 见 closure.toml 的完整声明
```

```toml
# store/sha256-<hash>/closure.toml
# 该包自身的直接依赖（传递闭包由 Closure 求解器展开）
[[requires]]
capability = "libssl.so.3"
constraint = ">=3.2, <4"

[[requires]]
capability = "libc"
constraint = "baseline-2.31+"
```

---

## 8. 验收清单(供未来实现自检)

- [ ] 内容规范化序列化稳定(同内容跨机 hash 一致)
- [ ] **语义权限位(可执行/setuid/setgid/sticky)纳入哈希输入,只归一 mtime 等噪声**
- [ ] **入库/取出显式恢复完整权限位,setuid 不丢**(PoC-6:天真复制会丢)
- [ ] **符号链接保留不解引用**(PoC-5)
- [ ] 写入走临时目录 + 原子 rename + 只读
- [ ] 加载期哈希校验,失败重拉
- [ ] 同包多版本并存验证
- [ ] 跨包文件去重(硬链接)对上层透明
- [ ] 预编译二进制可直接入 store(无需源码构建)
- [ ] meta.toml / closure.toml schema 完整
