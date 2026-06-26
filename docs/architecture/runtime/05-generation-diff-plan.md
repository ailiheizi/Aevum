# 世代 Diff / Plan —— 激活前看清"这次到底改了什么"

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:世代状态机 [`01-generation-lifecycle.md`](01-generation-lifecycle.md)、状态回滚 [`04-state-vs-package-rollback.md`](04-state-vs-package-rollback.md)、AI 维护 [`../../ai/01-maintainer-loop.md`](../../ai/01-maintainer-loop.md)
> 参考:`terraform plan`、`nix store diff-closures`

---

## 0. 设计哲学

> **改之前,先把"将要发生什么"完整摊给你看,你点头才执行。
> NixOS 有 diff-closures,但没人帮你读;Aevum 让 AI 用人话解释这次改动。**

这是激活前的知情关卡,也是 Aevum 最直接的 UX 优势之一。

---

## 1. 在状态机里的位置

Diff/Plan 插在 **verify 通过之后、activate 之前**:

```text
propose → verify ──► [ DIFF / PLAN 关卡 ] ──► activate
                          │
                          ├─ 计算候选世代 vs 当前 active 的差异
                          ├─ AI 用自然语言解释差异与影响
                          ├─ 标注风险(不可逆操作、大版本升级、保留两份)
                          └─ 等待确认(关键变更)/ 自动通过(平凡变更)
```

- verify 保证"这个世代是对的、能起来";plan 保证"用户知道这个世代改了什么"。两者职责不同,缺一不可。
- 对平凡、无风险、无代价的变更,plan 可配置为自动通过(只记录,不打断)。

---

## 2. Diff 计算什么

候选世代的 lock 与当前 active 的 lock 做结构化对比:

```text
对比维度：
  ├─ 包级:新增 / 移除 / 升级 / 降级(逐包,带版本与 hash)
  ├─ 闭包级:受影响的传递依赖(改一个底层库,谁跟着变)
  ├─ 层级:变更落在 Foundation / System / App 哪层
  ├─ 体积:净磁盘变化(要下载多少、释放多少)
  ├─ 状态:是否触发 state 快照/迁移(见 runtime/04)
  └─ 特殊:是否涉及"保留两份"、大版本升级、不可逆副作用
```

---

## 3. 原始 Diff(机器视图)

```text
$ aevum plan
Generation 7 → 8 (candidate)

Packages:
  + postgresql        16.2        (+45 MB)        [App]
  + libpq             16.2        (+2 MB)         [App]
  ~ openssl           3.0.13 → 3.2.1              [System]  ⚠ 12 包受影响
  - python            3.10.13     (-38 MB)        [App]
  ↺ openssl           保留两份: 3.0.13 + 3.2.1    [System]  ⚠

Closure:    142 → 145 packages
Disk:       +52 MB download, -38 MB freed, net +14 MB
Layers:     Foundation 不变 ✓ | System 1 项变更 | App 3 项变更
State:      postgresql 首次引入 → 将创建 state 子卷(无迁移)
```

这一层等价于 `nix store diff-closures` —— 准确,但需要懂行才能读。

---

## 4. AI 解释(人类视图)—— 关键差异化

AI 维护者把原始 diff 翻译成人话,点出"你真正该关心的":

```text
📋 这次变更:加入 PostgreSQL 数据库

  会发生什么:
  • 装上 PostgreSQL 16 和它的客户端库(约 45MB)
  • openssl 从 3.0 升到 3.2(系统层),12 个软件会用上新版

  ⚠ 需要你注意:
  • 有个老软件(app-Y)只兼容 openssl 3.0,所以系统会"保留两份"
    openssl:新软件用 3.2,app-Y 继续用 3.0(多占约 5MB)
  • python 3.10 被移除(没有软件再依赖它了)

  ✓ 安全的部分:
  • 系统核心(Foundation)完全不动,这次变更出问题可以秒回退
  • PostgreSQL 是首次安装,不涉及已有数据迁移

  [应用] [仅保存计划] [取消]
```

> 这正是 NixOS 缺的一环。diff-closures 给你一堆 store 路径变化,Aevum 给你"装了 PG、为了兼容老软件保留两份 openssl、核心没动可以放心"。理解成本从"得懂 Nix"降到"会读中文"。

---

## 5. 风险标注规则

AI 必须在 plan 中显式标注以下高风险项(对应各设计约束):

| 风险类 | 标注 | 依据 |
|---|---|---|
| 触碰 Foundation | 🚫 直接拒绝(verify 已挡) | [`../../layers/01-foundation.md`](../../layers/01-foundation.md) |
| 大版本升级有数据迁移 | ⚠ 迁移后回滚需 state 快照 | [`04-state-vs-package-rollback.md`](04-state-vs-package-rollback.md) |
| 不可逆副作用 | ⚠ 回滚无法撤销 | [`04-state-vs-package-rollback.md`](04-state-vs-package-rollback.md) §5 |
| 保留两份 | ⚠ 附磁盘代价 + 双份安全更新 | [`../../ai/02-repair-and-keep-two.md`](../../ai/02-repair-and-keep-two.md) |
| System 层大面积变更 | ⚠ 影响面与受影响包数 | [`../../layers/02-system-and-app.md`](../../layers/02-system-and-app.md) |

---

## 6. Plan 工件可保存

```text
aevum plan --save plan-gen-8.toml
```

- plan 可保存为工件,供审阅/审计/团队评审,稍后再 apply。
- 这呼应 terraform 的 `plan` → `apply` 分离:看过的计划和真正执行的计划一致(plan 锁定候选世代,apply 不重新求解)。
- 保存的 plan 关联候选世代 id;若期间 active 变了,apply 前重新校验并提示。

---

## 7. 与回滚的对称

Diff 不仅用于"前进"(激活新世代),也用于"后退":

```text
$ aevum plan --rollback 6
Generation 8 (active) → 6

  会发生什么(回退):
  • postgresql 16 → 移除(回到没装 PG 的状态)
  ⚠ postgresql 的数据将回滚到 gen-6 时的快照
    → gen-6 之后写入 PG 的数据会丢失!
  • openssl 回到单份 3.0.13

  [确认回退] [取消]
```

→ 回滚同样先 plan、先警告(尤其状态丢失),不盲目执行。

---

## 8. 边界

1. Plan 基于已 verified 的候选世代,**不重新求解**(看到的即将执行的)。
2. 平凡变更可自动通过,但**始终记录** plan 到世代历史(可事后审计)。
3. 高风险项(§5)**强制**进入需确认路径,不可静默 apply。
4. AI 解释是辅助,原始 diff 始终可查(透明,不只给"AI 说没事")。

---

## 9. 验收清单

- [ ] Diff 在 verify 后 activate 前计算
- [ ] 包级/闭包级/层级/体积/状态/特殊 六维对比
- [ ] 原始 diff(机器视图)准确,对标 diff-closures
- [ ] AI 自然语言解释,点出真正该关心的
- [ ] 五类风险强制标注
- [ ] plan 可保存为工件,apply 与所见一致
- [ ] 回滚同样先 plan + 状态丢失警告
- [ ] 原始 diff 始终可查(不只给 AI 结论)
