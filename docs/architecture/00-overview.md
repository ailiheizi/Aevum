# Aevum 架构总览

> 这是 Aevum 设计的主线入口。读完这一篇,你应该建立起整个系统的心智模型。
> 父文档:[`../README.md`](../../README.md) · 文档索引:[`../README.md`](../README.md)

---

## 0. 一句话

> **用户表达意图,Template 给出蓝图,Maintainer(AI)求解依赖链并构建世代,Store 不可变地存放,Generation 原子地激活与回滚,Layer 保证稳定层永不被软件层炸穿。**

Aevum 的目标:把 NixOS 的"可复现 + 原子化"留下,把"必须手写函数式 DSL"和"二进制生态摩擦"扔掉,用 AI 当依赖链的第一维护者。

---

## 1. 心智模型:从意图到运行

```text
   ┌─────────────────────────────────────────────────────────────────┐
   │  用户意图（自然语言 / 模板选择 / 声明式配置，无 DSL）              │
   │  "我要一个 Python 数据科学环境，带 GPU 支持"                       │
   └───────────────────────────────┬───────────────────────────────────┘
                                   │ 意图
                                   ▼
   ┌─────────────────────────────────────────────────────────────────┐
   │                      Maintainer（AI 维护者）                       │
   │  解析意图 → 选模板 → 求解依赖闭包 → 解冲突 → propose 世代          │
   │  propose → verify → activate → rollback 状态机的驱动者             │
   └───────────────┬─────────────────────────────────┬─────────────────┘
                   │ 解析出 Closure                    │ 写入/激活
                   ▼                                   ▼
   ┌───────────────────────────────┐   ┌─────────────────────────────────┐
   │   Store（内容寻址，不可变）    │   │   Generation（世代，可回滚）      │
   │   sha256-<hash>/ 每个包一份    │   │   gen-N/ 链接到一组特定 hash      │
   │   同包多版本并存               │   │   active → gen-N（原子指针）      │
   └───────────────┬───────────────┘   └─────────────────┬───────────────┘
                   │ 被引用                                │ 受约束于
                   ▼                                       ▼
   ┌───────────────────────────────────────────────────────────────────┐
   │              Layer（分层隔离） + Template（蓝图）                    │
   │   Foundation（密封核心，永不被软件层改）                            │
   │   System（系统层）  /  App（软件层，自由折腾）                       │
   └───────────────────────────────────────────────────────────────────┘
```

---

## 2. 六个核心对象

### 2.1 Store —— 内容寻址的不可变存储

所有包(二进制、库、配置产物)按内容哈希存放在 `store/sha256-<hash>/`。

- **不可变**:写入即只读。任何"修改"都是产生一个新 hash 目录。
- **多版本并存**:`python@3.11` 和 `python@3.12` 是两个不同 hash,互不干扰。
- **去重**:多个世代引用同一个 hash,磁盘上只有一份。
- **完整性**:加载时重算哈希比对,不匹配即报错并重新拉取。

详见 [`foundations/01-store.md`](foundations/01-store.md)。

### 2.2 Generation —— 不可变的系统世代

一次"变更"的结果,是一个完整的、不可变的系统快照。

- 每个世代是一组指向 store 内特定 hash 的链接 + 一份 lock 快照。
- **激活**是切换 `active` 原子指针,瞬间完成。
- **回滚**是把 `active` 指回上一个 verified 世代,秒级,无需重新求解。
- 旧世代永远保留(直到 GC 判定无引用),这是回滚和"保留两份"的基础。

详见 [`foundations/02-generation.md`](foundations/02-generation.md) 与状态机 [`runtime/01-generation-lifecycle.md`](runtime/01-generation-lifecycle.md)。

### 2.3 Layer —— 分层隔离(Aevum 相对 NixOS 的强化)

三层,从下到上稳定性递减、自由度递增:

| 层 | 内容 | 谁能改 |
|---|---|---|
| **Foundation** | 系统赖以启动和自愈的密封核心(init、Maintainer 自身、最小工具集) | 仅平台签名升级 |
| **System** | 系统级服务与共享运行时(显示、网络、驱动垫片、共享库基线) | AI 维护,谨慎变更 |
| **App** | 用户软件及其私有依赖 | 用户/AI 自由折腾 |

**关键保证**:App 层的依赖冲突、错装、删库,永远炸不穿 Foundation;最坏情况能进 foundation-only 最小可用系统自救。

详见 [`../layers/`](../layers/)。

### 2.4 Template —— 蓝图

模板是一份声明式的"系统/环境意图蓝图",不是图灵完备脚本。

- 内置模板:`minimal-desktop`、`dev-rust`、`dev-python-ds`、`server-web` 等。
- 用户可派生自定义模板,组合、覆盖。
- 选模板 = 给 Maintainer 一组高层意图,由它求解成具体闭包。

详见 [`../templates/`](../templates/)。

### 2.5 Maintainer —— AI 维护者

Aevum 区别于一切现有系统的核心。它是依赖链的第一维护者,驱动整个世代状态机:

- **propose**:把意图求解成候选世代(算闭包、选版本、解冲突)。
- **verify**:校验完整性、依赖一致性、可启动性。
- **activate**:验证通过才原子激活。
- **rollback**:出问题秒回上一个 verified 世代。
- **repair**:依赖坏了,自动诊断 → 生成多个修复方案 → 各自 verify → 择优;无解则**保留两份**隔离共存。

详见 [`../ai/`](../ai/)。

### 2.6 Closure —— 依赖闭包

一组意图求解出的完整依赖集合(传递闭包),即"要装哪些 hash"。Closure 是确定性求解器的输出,可复现:同样的意图 + 同样的索引快照 → 同样的 closure。

详见 [`foundations/03-closure.md`](foundations/03-closure.md)。

---

## 3. 三个文件层:Intent / Resolved / Lock

借鉴 NixOS 的声明/配置/lock 三段式,但去掉 DSL:

| 文件 | 是什么 | 谁写 | 类比 |
|---|---|---|---|
| **Intent** | 用户意图(模板引用 + 声明式覆盖,纯数据 TOML) | 用户 / AI | `configuration.nix`(但无逻辑) |
| **Resolved** | Maintainer 求解后的具体计划(选定的版本、来源) | AI 维护者 | 解析中间态 |
| **Lock** | 锁定的不可变快照(精确 hash + 闭包) | 系统生成 | `flake.lock` |

**可复现性的关键**:Lock 锁死一切。拿着同一份 Lock,在任何机器上重建出**逐字节一致**的世代。Intent 表达"想要什么",Lock 保证"得到的完全一致"。

详见 [`runtime/02-intent-resolved-lock.md`](runtime/02-intent-resolved-lock.md)。

---

## 4. 二进制兼容:NixOS 最大实操痛点的正面回应

NixOS 上下载的预编译二进制几乎跑不起来(`Could not start dynamically linked executable`),因为没有标准 `/lib`、动态链接器路径全是硬编码。Aevum 把这当**一等需求**:

- 提供稳定的动态链接器入口与共享库基线(在 System 层)。
- 二进制可声明所需的运行时基线,Maintainer 自动补齐到闭包。
- 目标:**普通 Linux 动态链接二进制开箱即跑**,不需要用户懂 patchelf。

详见 [`runtime/03-binary-compat.md`](runtime/03-binary-compat.md)。

---

## 5. 一次完整变更的生命周期(端到端)

```text
1. 用户:"加上 PostgreSQL 16"
        ↓
2. Maintainer 解析意图 → 命中 server-db 模板片段
        ↓
3. 求解 closure：postgres@16 + 它的传递依赖（按当前索引快照）
        ↓
4. 检查 store 缺哪些 hash → 下载补齐
        ↓
5. propose：创建 draft 世代 gen-(N+1)，写链接 + candidate lock
        ↓
6. verify：校验 hash 完整性、依赖闭合、Layer 约束、试启动
        ↓ 通过                              ↓ 失败
7. activate：原子切 active → gen-(N+1)     archive 该候选，active 不动
        ↓                                   ↓
8. 运行实例热重载                            Maintainer 进入 repair 流程
        ↓
9. 出问题？rollback：active 指回 gen-N（秒级）
```

详见 [`runtime/01-generation-lifecycle.md`](runtime/01-generation-lifecycle.md)。

---

## 6. 与 RELIK 的渊源

机制原型来自同作者的 RELIK 项目,它把"内容寻址 store + 世代 + propose/verify/activate/rollback + GC 引用计数 + Sealed Foundation"用在 **AI 插件系统**上。Aevum 把同一套理念**上升到系统级**,作为完整的 NixOS 替代。

---

## 7. 阅读顺序建议

1. 本文(架构总览)
2. [`adr/0001-positioning-vs-nixos.md`](adr/0001-positioning-vs-nixos.md) —— 定位边界
3. [`foundations/`](foundations/) —— Store / Generation / Closure / Template 对象模型
4. [`runtime/01-generation-lifecycle.md`](runtime/01-generation-lifecycle.md) —— 世代状态机
5. [`../layers/`](../layers/) —— 分层隔离
6. [`../ai/`](../ai/) —— AI 维护链路
7. [`../comparison/`](../comparison/) —— 凭什么比 NixOS 好
