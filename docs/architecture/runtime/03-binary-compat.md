# 二进制兼容 —— 正面回应 NixOS 最大的实操痛点

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:[`../foundations/01-store.md`](../foundations/01-store.md)、[`../../layers/02-system-and-app.md`](../../layers/02-system-and-app.md)、痛点出处 [`../../comparison/01-nixos-pain-points.md`](../../comparison/01-nixos-pain-points.md)

---

## 0. 设计哲学

> **普通 Linux 动态链接二进制,在 Aevum 上开箱即跑。用户不该需要懂 patchelf。**

这是 Aevum 对 NixOS 最高频实操痛点的正面回应,也是它"更易用"承诺里最硬的一块。

---

## 1. 痛点是什么

在 NixOS 上,你从网上下载一个预编译二进制,运行它,极大概率得到:

```text
$ ./some-app
bash: ./some-app: cannot execute: required file not found
# 或
Could not start dynamically linked executable: ./some-app
```

**根因**([出处见痛点文档](../../comparison/01-nixos-pain-points.md)):

1. 普通 Linux 二进制的 ELF interpreter 指向 `/lib64/ld-linux-x86-64.so.2`。
2. NixOS **没有** `/lib64`、没有标准 `/usr/lib` —— 一切都在 `/nix/store/<hash>-.../`。
3. 动态链接器找不到 → 直接拒绝启动。
4. 解法是 `nix-ld`、`patchelf --set-interpreter`、或包进 FHS 环境 —— 全是要求用户懂底层的折腾。

这把"想随便跑个下载来的工具"的普通用户挡在门外,是 NixOS 易用性的最大单点失败。

---

## 2. Aevum 的设计目标

| 目标 | 说明 |
|---|---|
| 标准 interpreter 可用 | 提供稳定的动态链接器入口,普通 ELF 的默认 interpreter 路径能被解析 |
| 共享库基线在场 | System 层提供一组稳定的共享库基线(libc、libstdc++、常见 .so),覆盖绝大多数二进制的运行时需求 |
| 缺库可声明可补齐 | 二进制可声明额外运行时需求,Maintainer 自动把它们求解进闭包 |
| 不牺牲可复现 | 兼容层本身也是世代化、内容寻址的 —— 不是往系统里乱塞全局库 |

> **关键平衡**:既要"二进制随便跑"(像传统发行版),又要"可复现 + 隔离"(像 NixOS)。下面的设计就是在这两者间取平衡。

---

## 3. 机制(设计草案)

> **实证**:[`PoC-2`](../../../poc/poc2-binary-compat/REPORT.md) 在真实 Linux(WSL Debian 13)上验证了本节机制的可行性 —— 把标准库路径用 tmpfs 遮蔽以复现 NixOS"无标准 /lib"困境后,裸跑 curl 退出码 127(`cannot execute`),而用 store 内 ld-linux + `--library-path` 启动**同一个未修改的二进制**退出码 0、正常运行。证明"显式 loader 入口让普通二进制开箱即跑、无需 patchelf"在底层成立;下面的设计是把它做成默认透明。

### 3.1 稳定链接器入口(System 层)

System 层在世代里提供一个稳定的动态链接器入口,并让标准 interpreter 路径(如 `/lib64/ld-linux-x86-64.so.2`)解析到它。

- 这个入口本身是 store 内一个被 System 层引用的 hash(可复现、可回滚)。
- 它的角色类似 NixOS 的 `nix-ld`,但**默认就在、默认配置好**,而非要用户主动启用。

### 3.2 共享库基线(runtime baseline)

System 层维护一个"运行时基线"包集合:

```toml
# 概念示意：system 层的 runtime baseline
[runtime_baseline]
glibc = "2.31+"            # 一个足够新的 glibc 基线
libstdcpp = "..."          # C++ 运行时
common = ["libz", "libssl", "libcurl", "libncurses", ...]
```

- 二进制运行时找 `libfoo.so` 时,链接器搜索路径包含基线提供的库。
- 基线版本被世代锁定 —— 升级基线 = 新世代 = 可回滚。

### 3.3 缺库的两条补齐路径

```text
二进制运行需要 libX.so，基线里没有
        ↓
路径 A（声明式，推荐）：
   包/模板声明 requires = ["libX.so >=N"]
   → Maintainer 求解进闭包 → 下个世代基线就带上
        ↓
路径 B（运行时探测，兜底）：
   首次运行检测到缺 libX.so
   → Maintainer 提议一个补齐基线的候选世代
   → verify → 用户确认 → activate
```

两条路径都**回到世代机制**,不往系统里塞全局污染。

### 3.4 与 Layer 的关系

- 链接器入口 + 运行时基线属于 **System 层**(见 [`../../layers/02-system-and-app.md`](../../layers/02-system-and-app.md))。
- App 层的二进制依赖这些基线,但**不能**通过改基线来满足自己的私有需求 —— 私有依赖进 App 层自己的闭包。
- Foundation 层不依赖兼容层(它用自己的最小静态工具),保证兼容层出问题也不影响系统自救。

---

## 4. 与 NixOS 的对照

| 维度 | NixOS | Aevum |
|---|---|---|
| 默认能跑普通二进制 | ❌ 默认不能 | ✅ 设计为默认能 |
| 启用方式 | 主动配 `nix-ld` / patchelf / FHS | 默认就配好 |
| 标准 interpreter 路径 | 不存在 | System 层提供 |
| 缺库怎么办 | 手动找 buildInputs / shell | 声明或探测 → Maintainer 补进闭包 |
| 是否牺牲可复现 | —— | 否,兼容层也世代化 |

---

## 5. 取舍与诚实声明

- **不是放弃纯净**:Aevum 不把库全局乱铺;基线是受控、世代化、可回滚的集合。
- **不可能 100%**:极端依赖(要求特定 glibc 旧版、专有驱动用户态组件)仍可能需要专门处理;但目标是覆盖绝大多数普通二进制,把"开箱即跑"变成默认而非例外。
- 本文是**设计意图**,具体 interpreter 注入、搜索路径构造的实现细节待代码阶段定稿并验证。

---

## 6. 验收清单(实现期)

- [ ] 标准 `/lib64/ld-linux-*.so.2` 路径可解析到 System 层链接器入口
- [ ] 常见预编译二进制(如下载的 CLI 工具)开箱即跑
- [ ] 运行时基线世代化、可回滚
- [ ] 缺库声明式补齐路径打通
- [ ] 缺库运行时探测兜底路径打通
- [ ] 兼容层故障不影响 foundation-only 启动
- [ ] 兼容层不引入全局可变状态(仍可复现)
