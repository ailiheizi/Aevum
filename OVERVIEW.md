# Aevum 项目总览(设计阶段收尾快照)

> 本文件是 Aevum 设计阶段的全局快照,给任何第一次接触本仓库的人(包括未来的作者)一份"我现在在哪"的地图。
> 最后更新:2026-06-09 · 阶段:**设计完成,待进入实现**

---

## 1. Aevum 是什么

AI-native、可复现、原子化的 Linux 用户态系统层 / 包管理器。对标 NixOS,但有两个根本差异:

- **更智能**:意图层用 AI + 模板 + 可选 TS 沙箱,而非强制 Nix 语言;依赖维护、冲突修复、回滚决策有 AI 参与(但 AI 是可选增强,非必需——见 PoC-3)。
- **更隔离**:Foundation / System / App 三层物理隔离,稳定层永不被软件层炸穿。

技术栈 Rust,许可证 Apache-2.0,建在 Linux 之上(复用宿主内核),主打开发者、兼顾桌面/服务器。

一句话理念:**用户表达意图,模板给蓝图,AI 维护者求解依赖链并构建不可变世代,内容寻址 store 存放,世代原子激活与回滚,分层隔离保稳定层不被炸穿。**

---

## 2. 现在到了哪一步

**设计阶段完成。** 产出:

- **顶层**:[`README.md`](README.md)(门面)、[`LICENSE`](LICENSE)(Apache-2.0)、本文件。
- **设计文档**:[`docs/`](docs/) 33 篇,含 5 个 ADR、对象模型、运行时、服务端、分层、模板、AI 维护、竞品对比。索引见 [`docs/README.md`](docs/README.md),从 [`docs/architecture/00-overview.md`](docs/architecture/00-overview.md) 读起。
- **实证**:[`poc/`](poc/) 7 个可运行 PoC(真实数据/真实 Linux),把核心假设逐一压过。
- **演进留痕**:[`docs/CHANGELOG.md`](docs/CHANGELOG.md) 记录九轮迭代(立项 → 语言前端 → 评审 → 五轮 PoC)。

---

## 3. 七个 PoC 各证了什么(核心)

每个 PoC 拔掉一个"纸上通、一做就炸"的风险点,都有真实数据/代码背书:

| PoC | 验证的核心假设 | 实测结论 |
|---|---|---|
| [1 索引可行性](poc/poc1-index-feasibility/REPORT.md) | 包元数据能否机器生成 | 仅 **11.6%** 纯自动、46.9% 人工语义 → **不自造生态,继承上游** |
| [2 二进制兼容](poc/poc2-binary-compat/REPORT.md) | 普通二进制能否开箱即跑 | 复现 NixOS 困境后,裸跑挂(rc127)、store loader 救活(rc0) |
| [3 零 AI 求解](poc/poc3-zero-ai-solver/REPORT.md) | 离了 AI 能不能装软件 | 442 真实 Debian 包零未解析、可复现 → **AI 是可选增强非门槛** |
| [4 多源隔离](poc/poc4-arch-isolation/REPORT.md) | Arch 等非自包含包能否消费 | 补闭包+轻隔离跑通;**铁律=同源补闭包(坑在 ABI 不在路径)** |
| [5 复杂包](poc/poc5-complex-pkg/REPORT.md) | python/imagemagick 会不会崩 | DT_NEEDED 补闭包不够,须**扫全包 ELF + 元数据**;符号链接须保留 |
| [6 架构盲区](poc/poc6-arch-edges/REPORT.md) | setuid/通信/磁盘 | setuid 须显式恢复权限位;轻隔离不挡通信;同版本去重省 **88%** |
| [7 核心机制](poc/poc7-core-mechanics/REPORT.md) | 世代/回滚/GC | 原子切换 0.09ms、回滚 0.095ms、**GC 不误删共享依赖** |

> 几个 PoC 抓到了真实现缺陷(补闭包漏 dlopen、内容寻址丢 setuid 位),都已回写进设计文档,赶在写代码前修正。

---

## 4. 生态策略(几轮讨论的结论)

Aevum **不自造包生态**,消费现有的:

```
来源:  Nix 包(首选,自包含)+ Arch/Debian(补闭包后纳入)
存储:  统一内容寻址 store(去重,多版本天然并存)
适配:  env 注入 / 显式 loader / patchelf(按包性质,默认不改二进制)
隔离:  默认轻隔离(库视图,不挡通信)+ 按需强隔离(namespace 沙箱)
铁律:  补闭包必须同源(ABI 自洽);复杂包须扫全包+元数据;符号链接保留
上层:  Aevum 世代 / 原子回滚 / AI 维护 / 分层隔离
```

详见 [`docs/architecture/foundations/04-index-and-supply.md`](docs/architecture/foundations/04-index-and-supply.md) 与 [`05-multi-source-and-isolation.md`](docs/architecture/foundations/05-multi-source-and-isolation.md)。

---

## 5. 五个不可违反的设计决策(ADR)

1. [0001](docs/architecture/adr/0001-positioning-vs-nixos.md) 定位为 Linux 之上的用户态系统层(不啃内核)。
2. [0002](docs/architecture/adr/0002-no-dsl-intent-layer.md) 意图层不强制图灵完备 DSL(纯数据 + 模板)。
3. [0003](docs/architecture/adr/0003-ai-maintainer-authority.md) AI 三边界:不直接选 hash、不动 Foundation、关键决策人类否决。
4. [0004](docs/architecture/adr/0004-typescript-intent-frontend.md) TS 作可选第二前端(沙箱求值,allowlist import)。
5. [0005](docs/architecture/adr/0005-ai-model-form-factor.md) AI 模型不进 Foundation、可插拔、重放不依赖模型。

---

## 6. 诚实的边界:还没做 / 还没证的

设计与可行性已充分,但以下属实现期或未验证,如实记录:

- **没有产品代码**:全部是设计 + Python/shell PoC,尚无 Rust 实现。
- **PoC 未覆盖**:强隔离完整 namespace 沙箱、setuid 包在沙箱下的真提权通道、多包大规模并发、跨机字节级复现、复杂包(dlopen/数据)补闭包的完整实现、同源上游完整依赖树拉取。
- **生态运营**:中心服务端(索引签名分发、缓存)的运营与可持续性是产品阶段课题。
- **采纳**:作者自用驱动,不依赖外部用户验证需求。

---

## 7. 下一步

文档侧收尾。从设计走向实现的自然第一步:

> **起 Rust workspace 骨架**——把七个 PoC 校正过的算法落成第一版能 `cargo build` 的代码:
> - `store`(内容寻址,含 setuid/权限位/符号链接正确处理)
> - `generation`(原子切换 / 瞬时回滚,PoC-7 已验证机制)
> - `solver`(确定性闭包求解,PoC-3 的 Python 逻辑translate 成 Rust)
> - `closure-builder`(补闭包:DT_NEEDED + 全包 ELF + 元数据,PoC-4/5 的算法)

实现期应同步补 [`docs/guides/`](docs/guides/)(构建/使用指南,当前占位)。

> **新会话接手提示**:项目根的 [`CLAUDE.md`](CLAUDE.md) 是 AI 协作者的第一必读(红线 + PoC 铁律 + 别重复做的事);Rust 实现的具体启动步骤见 [`docs/guides/01-rust-implementation-kickoff.md`](docs/guides/01-rust-implementation-kickoff.md)。

---

## 8. 一句话总结

> **Aevum 设计阶段完成:33 篇文档 + 7 个真实数据 PoC,把"消费现有生态 + AI 维护的可复现原子世代"从想法压到了有实测背书的设计。三大核心卖点(原子切换/回滚/GC)真文件验证全过,几个实现陷阱(dlopen 补闭包、setuid 权限位)已在写代码前抓出并修进设计。下一步是 Rust 实现。**
