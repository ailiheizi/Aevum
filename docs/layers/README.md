# 分层隔离 —— Foundation / System / App

> 父文档:[`../README.md`](../README.md)
> 子文档:[`01-foundation.md`](01-foundation.md)、[`02-system-and-app.md`](02-system-and-app.md)
> 关联:[`../architecture/foundations/02-generation.md`](../architecture/foundations/02-generation.md)

---

## 0. 设计哲学

> **稳定层与软件层物理隔离。系统赖以启动的核心被密封保护,普通软件再怎么折腾都炸不穿它。坏到极致,也能进最小可用系统自救。**

这是 Aevum 相对 NixOS 的核心强化。NixOS 的世代是扁平的一锅包;Aevum 把它切成稳定性递减的三层,让"系统永远能起来"成为结构性保证,而不是运气。

---

## 1. 三层模型

```text
   自由度 ▲                                          稳定性
         │   ┌──────────────────────────────────┐      │
   高     │   │  App 层                           │      │ 低
         │   │  用户软件 + 私有依赖              │      │
         │   │  自由装卸、AI 自由折腾、冲突隔离  │      │
         │   ├──────────────────────────────────┤      │
         │   │  System 层                        │      │
         │   │  共享运行时基线、链接器、系统服务 │      │
         │   │  AI 维护，谨慎变更                │      │
         │   ├──────────────────────────────────┤      │
   低     │   │  Foundation 层（密封核心）        │      │ 高
         │   │  init、Maintainer 自身、最小工具  │      │
         ▼   │  仅平台签名升级，普通操作不可改   │      ▼
             └──────────────────────────────────┘
```

| 层 | 内容 | 谁能改 | 坏了会怎样 |
|---|---|---|---|
| **Foundation** | 系统启动与自愈的密封核心 | 仅平台签名升级 | 系统起不来(被结构性禁止破坏) |
| **System** | 共享运行时、链接器、系统级服务 | AI 维护,谨慎 | 部分服务降级,可回滚 |
| **App** | 用户软件及私有依赖 | 用户 / AI 自由 | 仅影响该软件,隔离 |

---

## 2. 为什么要分层(对比 NixOS 的扁平世代)

NixOS 的一个世代是一整组包,`agent-core` 和你随手装的小工具在同一个平面。理论上世代回滚能救一切,但:

- 没有"哪些包神圣不可动"的**结构性**区分 —— 全靠约定和你自己别删错。
- 一次错误的大改可能让整个世代起不来,只能回滚(若回滚目标也被波及就麻烦)。

Aevum 的分层把"系统能起来"从"靠回滚兜底"升级成"靠结构保证":

1. **隔离故障半径**:App 层的冲突/错装,闭包上就和 Foundation/System 分离,炸不到下层。
2. **密封核心**:Foundation 被签名锁定,任何普通世代操作都不能删它、不能改它版本。
3. **最小可用兜底**:任何时候都能构造一个"只有 Foundation"的世代把系统拉起来。

---

## 3. 层与世代的结合

每个世代内部按层组织链接(见 [`../architecture/foundations/02-generation.md`](../architecture/foundations/02-generation.md) §3):

```text
gen-N/layers/
├── foundation/   → 永远在，版本被签名清单锁定
├── system/       → 共享基线，可随世代演进
└── app/          → 用户软件，世代间频繁变化
```

- 校验世代时,**逐层断言**:foundation 必须完整且版本精确,否则 verify 直接失败。
- 修复/回滚时,可只重算 app 层,保 foundation/system 不动 —— 修复更快、更安全。

---

## 4. 跨层规则(硬约束)

1. **依赖方向单向向下**:App 可依赖 System、Foundation;System 可依赖 Foundation;**反向禁止**。下层永不依赖上层,所以上层坏了下层照常。
2. **下层不为上层让步**:求解冲突时,绝不通过降级 Foundation/System 来迁就某个 App(见 [`../architecture/foundations/03-closure.md`](../architecture/foundations/03-closure.md) §5)。
3. **Foundation 只读于普通操作**:用户和 AI 的常规世代操作不能增删改 Foundation 包;只有"平台签名升级"这一条专门通道能动它(见 [`01-foundation.md`](01-foundation.md))。
4. **App 私有依赖不上浮**:App 需要某个特殊库版本,装进它自己的 App 层闭包,不污染 System 基线。

---

## 5. 子文档

- [`01-foundation.md`](01-foundation.md) —— 密封核心层:清单、签名、升级通道、极端兜底。
- [`02-system-and-app.md`](02-system-and-app.md) —— System 与 App 的职责边界、共享 vs 私有依赖、冲突隔离。

---

## 6. 验收清单

- [ ] 世代按三层组织,逐层可校验
- [ ] 依赖方向单向向下(编译期/求解期强制)
- [ ] 求解冲突不降级下层
- [ ] Foundation 普通操作只读
- [ ] App 私有依赖不污染 System 基线
- [ ] foundation-only 最小世代可启动
