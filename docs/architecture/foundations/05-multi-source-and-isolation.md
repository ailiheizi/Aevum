# 多源消费与隔离模型 —— 生态从哪来(实战版)

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:供给模型 [`04-index-and-supply.md`](04-index-and-supply.md)、store [`01-store.md`](01-store.md)、闭包 [`03-closure.md`](03-closure.md)、分层 [`../../layers/`](../../layers/)、二进制兼容 [`../runtime/03-binary-compat.md`](../runtime/03-binary-compat.md)
> 实证:[`PoC-2`](../../../poc/poc2-binary-compat/REPORT.md)(二进制兼容)、[`PoC-4`](../../../poc/poc4-arch-isolation/REPORT.md)(Arch 补闭包+隔离)

---

## 0. 设计哲学

> **不自己造生态,直接消费现有的(Nix/Arch/Debian…)。
> 不同来源的包统一进内容寻址 store,各带自己那一源的完整闭包,靠轻隔离划界 ——
> 于是它们都获得"多版本并存 + 原子回滚",哪怕原发行版不支持。**

这是 [`04-index-and-supply.md`](04-index-and-supply.md)"站在巨人肩上"战略的落地机制,经 PoC-2/PoC-4 实测验证。

---

## 1. 三类上游,两种性质

实测发现(PoC-2/PoC-4),上游包按"是否自包含"分两类,决定适配难度:

| 来源 | 找库方式 | 自包含 | 适配难度 |
|---|---|---|---|
| **Nix** (`/nix/store`) | RUNPATH 写死自己的闭包 | ✅ 是 | **低**:本就是内容寻址闭包,近乎拿来即用 |
| **Arch** (`.pkg.tar.zst`) | 标准路径 `/usr/lib` | ❌ 否 | 中:需补闭包(PoC-4 证全自动可行) |
| **Debian** (`.deb`) | 标准路径 `/lib` | ❌ 否 | 中:同 Arch |

→ **Nix 是首选(省力);Arch/Debian 经"补闭包"也能纳入。** 三者都最终落进 Aevum 统一的内容寻址 store。

---

## 2. 消费流程:导入 → 补闭包 → 入 store

```text
上游包(Nix/Arch/Debian)
        ↓ 1. 导入元数据(见 04-index-and-supply.md:依赖话术翻译进能力模型)
        ↓ 2. 补闭包
            ├─ Nix:已是闭包,直接采纳
            └─ Arch/Debian:解 ELF NEEDED,递归取齐依赖库,补成完整闭包
              ⚠ 复杂包还须额外补:运行时 dlopen 的插件 + 写死的数据路径(见 §3.5)
        ↓ 3. 内容寻址入 store(store/<hash>-<name>/,符号链接须保留不解引用)
        ↓ 4. 适配运行视图(见 §4)
   纳入 Aevum 世代,获得多版本并存 + 回滚 + AI 维护
```

PoC-4 实测:Arch ripgrep 递归补闭包 = rg + ld-linux + libc + libgcc_s + libpcre2,**全自动、零缺失**。简单 CLI 包成本低。

**但 PoC-5 证明:简单包的算法对复杂包不完整(见 §3.5)。**

---

## 3.5 ⚠️ 复杂包:DT_NEEDED 补闭包不完整(PoC-5 的关键教训)

PoC-5 用真实 python 3.14、imagemagick 7.1 压测,发现"只递归主二进制 DT_NEEDED"这套 PoC-4 算法**对复杂包会漏掉整片运行时依赖**:

```text
python3.14 主二进制 DT_NEEDED = { libpython3.14.so.1.0, libc.so.6 }
   ↓ 但运行时真正还需要(NEEDED 完全看不见):
     • 写死路径的标准库 /usr/lib/python3.14/(.py 模块)
     • 77 个 lib-dynload/*.so(import 时 dlopen)
     • _ssl.so 自己又 NEEDED libssl.so.3 + libcrypto.so.3(闭包深一层)
   ↓ 后果:python 能启动,`import ssl` 当场崩
imagemagick: 137 个编解码器插件(运行时 dlopen),只补主二进制 → 打开多数图片格式失败
```

根因:`DT_NEEDED` 是**链接期**依赖;复杂包大量用**运行时 dlopen**(按路径加载插件),这是 ELF 静态分析看不见的另一套机制。

**补闭包算法的修正(实现时必须从一开始就包含)**:

```text
完整补闭包 = ① 主二进制 DT_NEEDED 递归
           + ② 扫【整个包的所有 ELF】(插件/扩展),逐个解 NEEDED 并纳入其依赖
           + ③ 上游元数据声明的运行时目录(标准库 /usr/lib/pythonX/、插件目录)整体纳入
           + ④ 写死的数据路径(.py、配置)整目录纳入
```

→ **本质:补闭包不能只信 ELF 静态分析,要结合上游元数据声明的运行时结构。** 这是 [`04-index-and-supply.md`](04-index-and-supply.md)"必须继承上游元数据"的又一硬证据——标准库目录、插件路径只有上游标了,ELF 反推不出来。

---

## 3.6 ⚠️ 铁律:补闭包必须同源(PoC-4 的关键教训)

PoC-4 暴露出**整个多源方案最大的真实风险,且它不在大家以为的地方**:

```text
误以为的坑:多源包放一起 → 路径冲突
实测真相:  路径冲突隔离能解(已证);
           真正的坑是 ABI —— 给 Arch 包喂 Debian 的库,同名 .so ≠ ABI 兼容
           PoC-4 中 Arch rg 喂 Debian pcre2,报 "no version information available",
           换个对 glibc 符号敏感的程序就会 symbol-not-found 直接崩。
```

**铁律**:

> 一个包的闭包,必须从**它自己那一源**补齐(Arch 包带 Arch 的 glibc/pcre2,Nix 包带 Nix 的),**绝不跨源拼库**。

这不是限制,而是正确姿势——Nix 闭包、Flatpak runtime 都这么做。它也印证了 [`01-store.md`](01-store.md)"每个包绑定自己精确依赖 hash"的设计:同源闭包天然满足 ABI 自洽。

---

## 4. 隔离模型:分层(默认轻,按需强)

不同源的包要并存而不互相干扰,靠隔离。Aevum 采用**分层隔离**:

| 层级 | 做法 | 何时用 |
|---|---|---|
| **轻隔离(默认)** | 给每个程序私有的库搜索视图(env 注入 / 显式 loader / 绑定挂载),指向它自己在 store 的闭包 | 日常包,性价比最高 |
| **强隔离(按需)** | namespace 沙箱(文件/进程/网络隔离),类 Flatpak/distrobox | 不可信包、强安全边界服务 |

PoC-2/PoC-4 实测的是**轻隔离**:store 内 loader + 私有 `--library-path`,让非自包含的 Arch/Debian 二进制在无标准 `/lib` 环境跑通。轻隔离不开重容器、包间可通信、配合 store 去重不浪费磁盘。

> **实证(PoC-6/PoC-7)**:
> - 轻隔离只隔库搜索路径,**不隔进程/exec/pipe**,两个隔离包能正常互相调用(PoC-6 实测 rc=0)——默认隔离不破坏协作。
> - ⚠️ **强隔离的 setuid 边界**(PoC-7):user-namespace 内是"伪 root",真实 setuid 提权被 `no_new_privs` 限制。故**需要真提权的系统级包(如 sudo)不能塞进强隔离沙箱靠 setuid 工作**,要走 System 层 + 显式授权通道(特权 helper / polkit)。强隔离适合普通应用,不适合特权系统组件。

```text
内容寻址 store(底层共享,去重)
        +
每程序私有库视图(顶层隔离,各指向自己的同源闭包)
        =
无路径冲突(隔离) + 无 ABI 串味(同源闭包) + 不浪费磁盘(store 去重)
```

---

## 5. 三种轻隔离适配手段(按包性质选)

实测(PoC-2/PoC-4)可用的三招,从轻到重:

| 手段 | 做法 | 改二进制 | 能否共享官方缓存 |
|---|---|---|---|
| **env 注入** | 启动设 `LD_LIBRARY_PATH` 指向 store 闭包 | ❌ 不改 | ✅ 能 |
| **显式 loader** | `store/ld-linux --library-path <闭包> 程序` | ❌ 不改 | ✅ 能 |
| **patchelf 改写** | 把 interpreter/RUNPATH 写死成 store 路径 | ✅ 改(hash 变) | ❌ 不能 |

- 默认优先 **env 注入 / 显式 loader**(不改字节、可复现、能共享上游缓存)。
- 仅在需要"二进制永久自包含、脱离任何启动包装"时才用 **patchelf 改写**(代价:改写后与上游缓存字节不一致,成为 Aevum 自有条目)。

---

## 6. 多版本并存:store 天生能力

```text
python@3.11 → store/aaa-python/   (一个 hash)
python@3.12 → store/bbb-python/   (另一个 hash)
两者并存,各自闭包,隔离视图各指各的 → 天然多版本
```

- Nix 包:天生支持(hash 隔离)。
- Arch/Debian 包:原发行版不支持(都往 `/usr/lib` 塞会打架),**经 Aevum 补闭包 + 内容寻址后获得此能力**。

这正是"把不支持多版本的传统包,升级成内容寻址多版本"的价值,也接上 [`../../ai/02-repair-and-keep-two.md`](../../ai/02-repair-and-keep-two.md) 的"保留两份"。

---

## 7. 与现有设计的关系

- **不替代 [`04-index-and-supply.md`](04-index-and-supply.md),是它的运行时下半段**:04 讲"元数据怎么来",本篇讲"二进制怎么消费 + 隔离 + 并存"。
- **与分层架构一致**:强隔离对应 App 层对高风险包的加强([`../../layers/02-system-and-app.md`](../../layers/02-system-and-app.md))。
- **与世代模型的张力(诚实标注)**:"每程序隔离视图"与"一个世代描述整机"有张力。Aevum 的取法:**世代仍是整机可复现单元(记录所有包的 hash 闭包),隔离只是运行时如何呈现这些闭包的视图**,不改变"世代 = 整机快照"的语义。隔离视图由世代派生,不是各自为政的独立沙箱。

---

## 8. 边界与待办

- ✅ PoC-5 已测复杂包(python/imagemagick):发现 DT_NEEDED 补闭包对复杂包不完整(见 §3.5),算法已校正。setuid 包仍待测。
- 跨源 ABI 的同源补闭包,需能拉取上游完整依赖树(Arch repo / nixpkgs / Debian),本 PoC 只拉了单包本体。
- 强隔离(namespace 沙箱)未做 PoC,沿用 Flatpak/bubblewrap 成熟方案的假设待验证。
- 多包同时运行 + 包间通信的隔离边界未实测。

---

## 9. 验收清单

- [ ] 三类源(Nix/Arch/Debian)导入管线
- [ ] Arch/Debian 包递归补闭包,**同源取库**(不跨源)
- [ ] **复杂包:扫全包 ELF(插件/扩展)+ 纳入元数据声明的运行时目录/数据路径**(不只主二进制 NEEDED)
- [ ] **符号链接保留不解引用**(复杂包大量用,见 01-store)
- [ ] 内容寻址入 store,多版本并存
- [ ] 轻隔离三手段(env/loader/patchelf)按需选用
- [ ] 强隔离(namespace)作为高风险包的可选加强
- [ ] 隔离视图由世代派生,不破坏"世代=整机快照"语义
- [ ] ABI 自洽:闭包内库与二进制同源
