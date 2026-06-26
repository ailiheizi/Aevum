# AI 维护者(Maintainer)

> 父文档:[`../README.md`](../README.md)
> 子文档:[`01-maintainer-loop.md`](01-maintainer-loop.md)、[`02-repair-and-keep-two.md`](02-repair-and-keep-two.md)、[`03-garbage-collection.md`](03-garbage-collection.md)、[`05-intent-resolver-implementation.md`](05-intent-resolver-implementation.md)(实现设计)
> 关联:世代状态机 [`../architecture/runtime/01-generation-lifecycle.md`](../architecture/runtime/01-generation-lifecycle.md)、ADR [`../architecture/adr/0003-ai-maintainer-authority.md`](../architecture/adr/0003-ai-maintainer-authority.md)

---

## 0. 设计哲学

> **AI 是依赖链的第一维护者,人表达意图。
> 但 AI 在确定性求解器的笼子里工作 —— 它出主意、解冲突、做决策,具体闭包由求解器算,关键决策由人否决。**

这是 Aevum "比 NixOS 智能"的全部含义,也是它"智能而不失控、不失可复现"的边界所在。

---

## 1. Maintainer 是什么

一个常驻系统的 AI 维护者(本体在 Foundation 层,见 [`../layers/01-foundation.md`](../layers/01-foundation.md)),负责:

| 职责 | 说明 |
|---|---|
| **意图翻译** | 把"我要数据科学环境"翻译成明确的约束集 |
| **依赖维护** | 驱动 propose→verify→activate,维持系统依赖链健康 |
| **冲突修复** | 依赖冲突时提多个方案,各自验证,择优;无解则保留两份 |
| **回滚决策** | 出问题时判断回滚到哪个世代 |
| **GC 协助** | 按引用统计与保留策略,提议回收无用 hash |
| **解释** | 用 resolved.toml 的 reason/decision 让自己的决策可审计 |

---

## 2. 三个不可逾越的边界

Maintainer 强大,但被三条边界框住(详见 [`../architecture/adr/0003-ai-maintainer-authority.md`](../architecture/adr/0003-ai-maintainer-authority.md)):

1. **不直接选 hash**。AI 产出/调整约束与意图,具体"装哪个 hash"由**确定性求解器**计算。→ 保证可复现、可审计、可在无 AI 环境重放。
2. **不动 Foundation**。任何修复方案都不能增删改 Foundation 包。→ 保证系统永远能起来。
3. **关键决策需人类可否决**。AI 的决策写进 resolved.toml 的 `decision` 字段,重大取舍(如"保留两份""降级某软件")推到询问列表由用户拍板。→ 保证人在回路。

```text
        ┌──────────────── 人:表达意图、否决关键决策 ────────────────┐
        │                                                            │
        ▼                                                            │
   ┌─────────┐   约束/意图    ┌──────────────┐   精确闭包   ┌─────────┐
   │  人意图  │ ───────────► │  AI Maintainer │ ─────────► │ 确定性  │
   └─────────┘               │ (出主意/解冲突) │  调用      │ 求解器  │
                             └──────────────┘            └─────────┘
                                    │ 决策可审计(resolved.toml)
                                    ▼
                          propose→verify→activate→rollback
```

---

## 3. 与"AI 助手"路线的区别

| | nixai 式 AI 助手 | Aevum Maintainer |
|---|---|---|
| 角色 | 旁边的顾问,帮你写配置/解释报错 | 系统的第一维护者,直接驱动世代 |
| 是否动系统 | 不动,只给建议 | 在边界内直接 propose/verify/activate |
| 求解 | 还是你跑 nix | AI 翻译意图 + 求解器算闭包 |
| 出错 | 你自己回滚 | AI 自动诊断 + 提修复方案 + 可秒回 |

Aevum 不是"给系统配个会说话的助手",而是"让 AI 成为系统的运维主体,但锁在确定性 + 分层 + 人类否决的笼子里"。

---

## 4. 子文档

- [`01-maintainer-loop.md`](01-maintainer-loop.md) —— 维护循环:意图解析、求解、健康维持。
- [`02-repair-and-keep-two.md`](02-repair-and-keep-two.md) —— 冲突修复策略与"保留两份"。
- [`03-garbage-collection.md`](03-garbage-collection.md) —— GC:引用统计与回收。

---

## 5. 验收清单

- [ ] Maintainer 本体在 Foundation 层
- [ ] AI 只产约束/意图,不直接选 hash
- [ ] AI 任何方案都不动 Foundation
- [ ] 关键决策写入 resolved.decision 并可被人否决
- [ ] 修复全失败时系统停在可用态
