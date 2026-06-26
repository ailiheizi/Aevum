# 维护循环 —— 意图解析、求解、健康维持

> 父文档:[`README.md`](README.md)
> 关联:世代状态机 [`../architecture/runtime/01-generation-lifecycle.md`](../architecture/runtime/01-generation-lifecycle.md)、闭包 [`../architecture/foundations/03-closure.md`](../architecture/foundations/03-closure.md)

---

## 0. 目的

描述 Maintainer 日常的工作循环:从一个意图到一个激活的世代,以及它如何持续维持系统依赖链健康。

---

## 1. 主循环

```text
          ┌─────────────────────────────────────────────┐
          │              等待触发                         │
          │  用户意图 / 健康检查告警 / 升级通知 / 定时巡检 │
          └───────────────────┬─────────────────────────┘
                              ▼
                    ┌──────────────────┐
                    │ 1. 意图解析       │  自然语言/模板/配置 → 约束集
                    └─────────┬────────┘
                              ▼
                    ┌──────────────────┐
                    │ 2. 求解闭包       │  调确定性求解器（AI 不直接选 hash）
                    └─────────┬────────┘
                         ┌────┴────┐
                    无冲突 │         │ 有冲突
                         ▼         ▼
              ┌──────────────┐  ┌──────────────────────┐
              │ 3a. propose  │  │ 3b. repair（见 02 篇）│
              │   候选世代    │  │  多方案 → 各自 propose │
              └──────┬───────┘  └──────────┬───────────┘
                     └──────────┬──────────┘
                                ▼
                       ┌──────────────────┐
                       │ 4. verify         │  完整性/闭合/层约束/可启动
                       └─────────┬────────┘
                            ┌────┴────┐
                       通过  │         │ 失败
                            ▼         ▼
                  ┌──────────────┐  ┌──────────────┐
                  │ 5. 推荐/激活  │  │  archive，    │
                  │  关键决策问人 │  │  active 不动  │
                  └──────┬───────┘  └──────────────┘
                         ▼
                  ┌──────────────┐
                  │ 6. activate   │  原子切换 + 热重载
                  └──────┬───────┘
                         ▼
                  ┌──────────────┐
                  │ 7. 观察       │  出问题 → rollback（秒回）
                  └──────────────┘
```

---

## 2. 各步细节

### 2.1 意图解析

输入可能是:

- 自然语言("帮我把开发环境换成 Python 3.11")。
- 模板选择(选 `dev-python-ds`)。
- 直接编辑 intent.toml。

AI 把它统一成**明确约束集**,产出 resolved.toml 的初稿(含 reason)。模糊处由 AI 补全,但补全本身也写进 reason 供审计。

### 2.2 求解闭包

把约束集交给**确定性求解器**(见 [`../architecture/foundations/03-closure.md`](../architecture/foundations/03-closure.md)):

- AI 不在这一步"选包" —— 它只提供约束。
- 求解器优先复用 store 已有 hash(省下载、利去重)。
- 输出精确闭包或冲突报告。

### 2.3 propose / 2.3b repair

- 无冲突 → 直接 propose 一个候选世代。
- 有冲突 → 进 repair 流程,可能 propose 多个候选(见 [`02-repair-and-keep-two.md`](02-repair-and-keep-two.md))。

### 2.4 verify

四类校验全过才算 verified(见状态机文档 §3):完整性、闭合性、层约束、可启动性。

### 2.5 推荐 / 关键决策问人

- 普通变更:verified 即可激活(可配置为自动或确认)。
- 关键决策(保留两份、降级某软件、System 基线升级):推到"AI 询问列表",附 resolved.decision 说明,由用户拍板。

### 2.6 activate

原子切 active + 通知热重载(见状态机文档 §4)。

### 2.7 观察与回滚

激活后短期观察(可选健康探针):

```text
激活后出现异常（服务起不来 / 关键探针失败）
        ↓
Maintainer 判断 → 自动或建议 rollback 到上一 verified 世代（秒回）
        ↓
记录失败原因，避免下次重蹈
```

---

## 3. 持续健康维持(被动触发之外)

Maintainer 不只响应用户,还主动维持系统健康:

| 触发 | 动作 |
|---|---|
| 安全更新可用 | 提议把受影响包升级到修复版(走完整 propose→verify) |
| Foundation 升级通知 | 推到询问列表(见 [`../layers/01-foundation.md`](../layers/01-foundation.md) §5) |
| 定时巡检发现闭包不一致 | 诊断并提议修复 |
| 磁盘超阈值 | 提议 GC(见 [`03-garbage-collection.md`](03-garbage-collection.md)) |

所有主动动作同样遵守三边界:不直接选 hash、不动 Foundation、关键决策问人。

---

## 4. 决策透明性

Maintainer 的每个非平凡决策都落在 resolved.toml:

```toml
[[decision]]
topic = "python 版本选择"
chosen = "3.11.8"
reason = "intent 要求 3.11；3.11.8 是该线最新安全补丁"
alternatives = ["3.11.7（有已知 CVE，否决）"]
needs_user_confirm = false

[[decision]]
topic = "openssl 冲突"
chosen = "保留两份"
reason = "app-X 需 3.2，app-Y 仅兼容 3.0，无共存单版本"
alternatives = ["统一 3.2（Y 崩溃）", "统一 3.0（X 缺特性）"]
needs_user_confirm = true          # 推到询问列表
```

→ 用户随时能看懂"AI 为什么这么做",并在 `needs_user_confirm` 的项上否决。

> **重要(防 AI 自我放行)**:`needs_user_confirm` 是 AI 的**自述**,系统**不单独信任它**。verify 阶段会**独立、机器判定**是否命中高危 CVE 或版本回退,命中即强制人工确认,无论 AI 把该字段标成什么(见 [`../architecture/runtime/01-generation-lifecycle.md`](../architecture/runtime/01-generation-lifecycle.md) §3.1,回应评审 H4)。即 AI 可以建议"无需确认",但无权让危险变更绕过独立检查。

---

## 5. 验收清单

- [ ] 主循环七步完整
- [ ] 意图解析产出可审计 resolved 初稿
- [ ] 求解步 AI 不直接选 hash
- [ ] 关键决策推询问列表 + 可否决
- [ ] 激活后观察 + 异常回滚
- [ ] 主动健康维持(安全更新/巡检/GC)遵守三边界
- [ ] 所有非平凡决策写入 resolved.decision
