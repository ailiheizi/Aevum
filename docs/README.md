# Aevum 文档体系

> Aevum 的全部设计文档。当前处于**设计阶段**,本目录是唯一交付物。
> 项目门面见 [`../README.md`](../README.md)。

---

## 从这里开始

1. **[`architecture/00-overview.md`](architecture/00-overview.md)** —— 架构总览,先读这一篇建立心智模型。
2. **[`comparison/01-nixos-pain-points.md`](comparison/01-nixos-pain-points.md)** —— 我们到底在解决什么问题。
3. **[`architecture/adr/0001-positioning-vs-nixos.md`](architecture/adr/0001-positioning-vs-nixos.md)** —— Aevum 的边界与定位。

---

## 按主题导航

### 架构主线 `architecture/`

- [`00-overview.md`](architecture/00-overview.md) —— 架构总览
- **ADR(架构决策记录,不可违反)** `architecture/adr/`
  - [`0001-positioning-vs-nixos.md`](architecture/adr/0001-positioning-vs-nixos.md) —— 定位:对标 NixOS 的用户态系统层
  - [`0002-no-dsl-intent-layer.md`](architecture/adr/0002-no-dsl-intent-layer.md) —— 意图层不引入图灵完备 DSL
  - [`0003-ai-maintainer-authority.md`](architecture/adr/0003-ai-maintainer-authority.md) —— AI 维护者的权限边界与人类否决权
  - [`0004-typescript-intent-frontend.md`](architecture/adr/0004-typescript-intent-frontend.md) —— 意图层增加 TypeScript 可选第二前端(沙箱求值)
  - [`0005-ai-model-form-factor.md`](architecture/adr/0005-ai-model-form-factor.md) —— AI 模型形态与离线降级(回应评审 H5)
- **核心对象模型** `architecture/foundations/`
  - [`01-store.md`](architecture/foundations/01-store.md) —— 内容寻址存储
  - [`02-generation.md`](architecture/foundations/02-generation.md) —— 世代模型
  - [`03-closure.md`](architecture/foundations/03-closure.md) —— 依赖闭包与求解
  - [`04-index-and-supply.md`](architecture/foundations/04-index-and-supply.md) —— 包索引与供给模型(生态从哪来,回应评审 C1)
  - [`05-multi-source-and-isolation.md`](architecture/foundations/05-multi-source-and-isolation.md) —— 多源消费与隔离模型(消费 Nix/Arch/Debian,PoC-2/4 实证)
- **运行时机制** `architecture/runtime/`
  - [`01-generation-lifecycle.md`](architecture/runtime/01-generation-lifecycle.md) —— 世代状态机(propose/verify/activate/rollback)
  - [`02-intent-resolved-lock.md`](architecture/runtime/02-intent-resolved-lock.md) —— 三文件层:意图/求解/锁定(TOML / 自然语言 / TS 三前端)
  - [`03-binary-compat.md`](architecture/runtime/03-binary-compat.md) —— 普通二进制兼容
  - [`04-state-vs-package-rollback.md`](architecture/runtime/04-state-vs-package-rollback.md) —— 状态 vs 包回滚(可变数据协调回退)
  - [`05-generation-diff-plan.md`](architecture/runtime/05-generation-diff-plan.md) —— 世代 diff/plan(激活前看清改了什么)
  - [`06-remote-cache.md`](architecture/runtime/06-remote-cache.md) —— 远程缓存与增量传输
  - [`07-host-coexistence.md`](architecture/runtime/07-host-coexistence.md) —— 宿主共存(PATH 投影/不碰宿主/干净卸载,回应 H7)
- **服务端与信任** `architecture/server/`
  - [`01-server-and-trust-root.md`](architecture/server/01-server-and-trust-root.md) —— 中心服务端(索引/缓存/Foundation 通道)+ 信任根(回应 H1/H2)

### 分层隔离 `layers/`

- [`README.md`](layers/README.md) —— 分层总览:Foundation / System / App
- [`01-foundation.md`](layers/01-foundation.md) —— 密封核心层(稳定性兜底)
- [`02-system-and-app.md`](layers/02-system-and-app.md) —— 系统层与软件层的隔离

### 模板系统 `templates/`

- [`README.md`](templates/README.md) —— 模板是什么、怎么用
- [`01-template-model.md`](templates/01-template-model.md) —— 模板数据模型与派生/覆盖

### AI 维护者 `ai/`

- [`README.md`](ai/README.md) —— AI 维护者总览
- [`01-maintainer-loop.md`](ai/01-maintainer-loop.md) —— 维护循环:解析→求解→修复
- [`02-repair-and-keep-two.md`](ai/02-repair-and-keep-two.md) —— 冲突修复与"保留两份"
- [`03-garbage-collection.md`](ai/03-garbage-collection.md) —— GC:引用统计与回收
- [`04-reconciliation-loop.md`](ai/04-reconciliation-loop.md) —— 调和循环:Maintainer 作为自治收敛器

### 对比与竞品 `comparison/`

- [`01-nixos-pain-points.md`](comparison/01-nixos-pain-points.md) —— NixOS 痛点剖析(带出处)
- [`02-prior-art.md`](comparison/02-prior-art.md) —— 竞品与既有工作(osModa / nixai / 不可变发行版)

### 指南 `guides/`

- [`01-rust-implementation-kickoff.md`](guides/01-rust-implementation-kickoff.md) —— Rust 实现启动指南(四个 crate / 对应 PoC / 实现铁律 / 第一个里程碑)
- 构建/使用指南待代码骨架落地后补。

---

## 文档约定

- 所有文档用中文写,代码/数据结构保留英文标识符。
- 每篇文档顶部标注父文档与关联文档,便于双向跳转。
- 涉及外部事实(NixOS 行为、竞品)的论断附出处链接。
- 设计变更记入 [`CHANGELOG.md`](CHANGELOG.md)。

---

## 文档状态

| 区块 | 状态 |
|---|---|
| README / 架构总览 / 索引 | ✅ 已成稿 |
| ADR(0001–0005) | ✅ 已成稿 |
| foundations(对象模型,含 04 供给 / 05 多源隔离) | ✅ 已成稿 |
| runtime(运行时机制 01–07) | ✅ 已成稿 |
| server(服务端 + 信任根) | ✅ 已成稿 |
| layers(分层隔离) | ✅ 已成稿 |
| templates(模板) | ✅ 已成稿 |
| ai(AI 维护者,含调和循环 / GC) | ✅ 已成稿 |
| comparison(对比竞品) | ✅ 已成稿 |
| **PoC 实证(7 个,见 `../poc/`)** | ✅ 全部完成,核心假设有真实数据/代码背书 |
| 项目总览 [`../OVERVIEW.md`](../OVERVIEW.md) | ✅ 已成稿 |
| guides(指南) | ⬜ 待代码骨架后补 |

> 设计阶段交付完成。下一步从设计走向实现(Rust workspace 骨架)。各核心假设的实测结论见 [`../poc/`](../poc/) 七个 PoC 报告,汇总见 [`../OVERVIEW.md`](../OVERVIEW.md)。
