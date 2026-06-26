# 竞品与既有工作

> 父文档:[`../README.md`](../README.md)
> 关联:NixOS 痛点 [`01-nixos-pain-points.md`](01-nixos-pain-points.md)、定位 ADR [`../architecture/adr/0001-positioning-vs-nixos.md`](../architecture/adr/0001-positioning-vs-nixos.md)

---

## 0. 为什么要写这篇

开工前先确认:**有没有人已经做过 Aevum 想做的事?** 答案是 —— 有人在相邻赛道,但没人占据 Aevum 的精确位置。这篇厘清竞品边界,确保 Aevum 不是重复造轮子,也不沦为某个已有项目的套皮。

---

## 1. 赛道地图

```text
                     是否用 AI 维护系统/依赖？
                  否                          是
              ┌────────────────────┬────────────────────────┐
  可复现/    │ NixOS / Guix        │ ★ Aevum（目标位置）     │
  内容寻址   │ （DSL，无 AI）       │ （无 DSL，AI 维护依赖链） │
  世代       ├────────────────────┼────────────────────────┤
              │ Vanilla OS/blendOS │ osModa                  │
  镜像级     │ Silverblue 等       │ （NixOS 之上加 AI 大脑） │
  不可变     │ （原子但非细粒度）   │                         │
              └────────────────────┴────────────────────────┘
                                    nixai（只是助手，不改 OS）
```

---

## 2. 逐个竞品分析

### 2.1 osModa —— 最接近的竞品

**是什么**:自称"首个 AI-native 操作系统",**建在 NixOS + Rust 之上**。9 个 Rust 守护进程、80+ 系统工具、自愈、P2P mesh、防篡改审计链,复用 NixOS 的原子回滚。

来源:
- [osModa Review (avenacloud)](https://avenacloud.com/blog/osmoda-ai-managed-operating-system-servers/)
- [osModa Review 2026 (coronium)](https://www.coronium.io/partners/hosting/osmoda-review)
- [osModa Spawn 指南](https://www.coronium.io/blog/osmoda-spawn-ai-agent-skills)
- 官网 [os.moda](https://os.moda/)

**和 Aevum 的关键区别**:

| 维度 | osModa | Aevum |
|---|---|---|
| 与 NixOS 关系 | **建在 NixOS 之上**,是它的 AI 增强层 | **替代** NixOS 的意图层,自成体系 |
| 是否保留 Nix 语言 | 保留(底下还是 Nix) | **不保留**,无 DSL |
| 二进制摩擦 | 继承 NixOS 的(没解决) | 当一等需求正面解决 |
| 主打场景 | 服务器 / AI agent 托管基础设施 | **先桌面**、兼顾服务器 |
| AI 角色 | 自愈、运维、自然语言控制服务器 | 依赖链第一维护者 + 自愈 + 模板 |

**结论**:osModa 证明了"AI-native OS"方向有人买单,是重要的信号。但它选择"在 NixOS 上加 AI",因此**继承了 Nix 语言和二进制摩擦这两个最大痛点**。Aevum 的差异化正是把意图层本身换掉,并主打桌面/开发者。

### 2.2 nixai —— AI 助手,不改 OS

**是什么**:NixOS 的 AI 助手 CLI,帮你写配置、解释报错,接多个 AI 提供商。

来源:[Introducing nixai](https://discourse.nixos.org/t/introducing-nixai-your-ai-powered-nixos-companion/65168)

**区别**:nixai 是"坐在终端里的 NixOS 专家",**不改变 NixOS 本身**,你还是得跑 nix、写 Nix。Aevum 的 Maintainer 是系统运维主体,直接驱动世代。两者角色根本不同(详见 [`../ai/README.md`](../ai/README.md) §3)。

### 2.3 镜像级不可变发行版(Vanilla OS / blendOS / Silverblue / MicroOS)

**是什么**:一批 immutable / atomic Linux 发行版,只读系统分区 + 事务式更新 + 失败回滚。

来源:
- [9 atomic/immutable Linux distros 2025](https://linuxbsdos.com/2025/05/04/9-atomic-or-immutable-linux-distributions/)
- [Atomic vs immutable Linux (ZDNet)](https://www.zdnet.com/article/atomic-vs-immutable-linux-distro-how-to-decide/)
- [The Immutable Linux Paradox](https://jnsgr.uk/2025/09/immutable-linux-paradox)

**区别**:它们的原子性是**镜像/分区级**(整个系统镜像换一版),不是 Aevum/NixOS 的**包级细粒度内容寻址**。它们通常:

- 没有"同包多版本并存 + 细粒度回退到任意世代"。
- 没有 AI 维护依赖链。
- 软件靠容器/Flatpak 旁路,而非统一的可复现闭包。

Aevum 要的是**细粒度内容寻址世代 + AI 维护**,比镜像级不可变精细得多。

### 2.4 Guix —— NixOS 的近亲

**是什么**:和 Nix 同源理念,用 Guile Scheme 作配置语言。

**区别**:把 Nix 语言换成 Scheme,**DSL 的墙还在**(甚至要求懂 Lisp),没有 AI 维护。Aevum 的方向相反 —— 取消 DSL。

### 2.5 学术方向:AIOS / agent OS

**是什么**:arxiv / [agiresearch/AIOS](https://github.com/agiresearch/AIOS) 等,研究"给 AI agent 用的操作系统"(LLM 作为调度核心,管理 agent 的内存/工具/上下文)。

来源:[The AI-Native OS: Rethinking the OS from First Principles](https://medium.com/@yashash.gc/the-ai-native-os-rethinking-the-operating-system-from-first-principles-a2b5c02332a6)

**区别**:这是"OS for AI agents"(让 OS 服务于 agent),Aevum 是"AI maintains the OS"(让 AI 维护系统依赖)。同样有 "AI" 和 "OS",目标正交。

---

## 3. 空档结论

把所有竞品叠在一起,有一个**没有人占据的精确位置**:

> **AI 作为依赖链第一维护者 + 内容寻址细粒度世代 + 无 DSL 意图层 + 普通二进制友好 + 模板化分层隔离,主打桌面/开发者。**

- osModa 占了"AI-native OS",但建在 NixOS 上,留着 Nix 语言和二进制摩擦,且主打服务器。
- nixai 只是助手。
- 不可变发行版只有镜像级原子,没有 AI、没有细粒度世代。
- Guix 还是 DSL。
- AIOS 是给 agent 用的 OS,目标正交。

**Aevum 的位置是空的,且 osModa 已经验证了方向的市场信号。** 这是开工的依据。

---

## 4. 我们要警惕的(避免重蹈)

- 别变成"另一个 osModa":Aevum 的意图层必须真的去掉 DSL,而不是在 Nix 上糊层 AI。
- 别变成"另一个 NixOS":二进制兼容、模板易用性必须是一等公民,不能事后打补丁。
- 别变成"只是个助手":Maintainer 要真的驱动世代,而不是只给建议。

---

## 参考来源

- [osModa (avenacloud)](https://avenacloud.com/blog/osmoda-ai-managed-operating-system-servers/) · [osModa (coronium)](https://www.coronium.io/partners/hosting/osmoda-review) · [os.moda](https://os.moda/)
- [nixai](https://discourse.nixos.org/t/introducing-nixai-your-ai-powered-nixos-companion/65168)
- [9 atomic/immutable Linux distros 2025](https://linuxbsdos.com/2025/05/04/9-atomic-or-immutable-linux-distributions/)
- [The Immutable Linux Paradox](https://jnsgr.uk/2025/09/immutable-linux-paradox)
- [agiresearch/AIOS](https://github.com/agiresearch/AIOS)
- [The AI-Native OS (Medium)](https://medium.com/@yashash.gc/the-ai-native-os-rethinking-the-operating-system-from-first-principles-a2b5c02332a6)
