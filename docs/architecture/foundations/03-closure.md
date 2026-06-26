# Closure —— 依赖闭包与求解

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:[`01-store.md`](01-store.md)、[`02-generation.md`](02-generation.md)、AI 求解 [`../../ai/01-maintainer-loop.md`](../../ai/01-maintainer-loop.md)

---

## 0. 设计哲学

> **意图说"我要什么",闭包说"那需要哪些 hash 的完整集合"。
> 求解是确定性的:同样的意图 + 同样的索引快照 → 同样的闭包。**

闭包是连接"高层意图"和"具体 store hash"的桥梁。它是可复现性的逻辑保证(store 是物理保证)。

---

## 1. 是什么

Closure(闭包)= 一组意图求解出的**完整传递依赖集合**。

- 输入:意图(模板引用 + 声明式覆盖)+ 索引快照(可用包及其版本/依赖关系)。
- 输出:一组精确的 store hash —— "要装这些,一个不多一个不少"。
- "传递":如果 A 依赖 B,B 依赖 C,闭包包含 A、B、C 全部。

```text
意图：dev-python-ds 模板
        ↓ 求解
closure = {
  sha256-aaa  python@3.11.8
  sha256-bbb  numpy@1.26.4
  sha256-ccc  pandas@2.2.1
  sha256-ddd  openblas@0.3.26   ← numpy 的传递依赖
  sha256-eee  libc-baseline     ← 运行时基线
  ...
}
```

---

## 2. 谁来求解:确定性求解器 + AI 维护者

Aevum 的求解分两层,各司其职 —— 这是它"比 NixOS 智能"又"不失可复现"的关键平衡:

| 层 | 角色 | 性质 |
|---|---|---|
| **确定性求解器** | 给定版本约束,计算满足约束的具体闭包 | 纯确定性、可复现、可审计 |
| **AI 维护者** | 把模糊意图翻译成约束;约束无解时探索取舍方案 | 启发式、需人类确认关键决策 |

**分工原则**:
- AI **不**直接拍板"装哪个 hash"。它产出/调整**约束与意图**,再交给确定性求解器算闭包。
- 这样既保留了 AI 的灵活(理解"我要数据科学环境"),又保证了求解结果可复现、可审计、可在无 AI 环境重放。
- 关键边界记录在 [`../adr/0003-ai-maintainer-authority.md`](../adr/0003-ai-maintainer-authority.md)。

```text
模糊意图 ──AI──► 明确约束集 ──确定性求解器──► closure（精确 hash 集）
"数据科学"      python3 + numpy           {sha256-...}
                + pandas + ...
```

---

## 3. 求解过程

```text
1. 收集约束
   ├─ 模板带来的约束（dev-python-ds → python3, numpy, pandas...）
   ├─ 用户覆盖（"python 要 3.11 不要 3.12"）
   └─ Layer 约束（foundation 包版本被锁死，不可被求解动摇）
        ↓
2. 版本选择
   ├─ 在索引快照内，为每个能力选满足约束的版本
   └─ 优先复用 store 中已有的 hash（减少下载、利于去重）
        ↓
3. 传递展开
   └─ 递归展开每个包的 requires，直到闭合（无未满足依赖）
        ↓
4. 冲突检测
   ├─ 无冲突 → 输出 closure
   └─ 有冲突 → 交给 AI 维护者的 repair 流程（见 §5）
        ↓
5. 闭包指纹
   └─ 对最终 hash 集排序后取摘要 = closure_id（同闭包 → 同 id，写入世代 manifest）
```

---

## 4. 冲突的本质

冲突 = 两条约束无法同时满足。最常见:

```text
软件 X 要求 openssl >=3.2
软件 Y 要求 openssl <3.2（只兼容 3.0.x）
        ↓
确定性求解器：无单一 openssl 版本同时满足 → 冲突
```

---

## 5. 冲突的三种归宿

当确定性求解器报冲突,AI 维护者介入,按优先级尝试:

```text
方案 A：调整约束求共存版本
        （能否找到一个 openssl 同时满足 X 和 Y 的真实运行需求？
         有时约束声明过严，AI 评估实际兼容性后放宽）
        ↓ 不行
方案 B：升级/降级一方
        （把 Y 升级到兼容 openssl 3.2 的新版本）
        ↓ 不行
方案 C：保留两份，隔离共存  ←── Aevum 的兜底，store 多版本能力使其可行
        （X 的闭包引用 openssl@3.2 的 hash，
         Y 的闭包引用 openssl@3.0 的 hash，
         两个版本在 store 中并存，互不干扰）
```

- 每个方案都走完整 propose → verify,只有 verify 通过的才推给用户选择。
- **Foundation 层永远不参与让步**:AI 不能为了让某软件跑通而降级 foundation 包(见 [`../../layers/01-foundation.md`](../../layers/01-foundation.md))。
- "保留两份"的完整设计见 [`../../ai/02-repair-and-keep-two.md`](../../ai/02-repair-and-keep-two.md)。

---

## 6. 可复现性保证

闭包可复现依赖三个锁定:

1. **索引快照锁定** —— 求解时用的"有哪些包、什么版本、什么依赖关系"被快照固定(写入 lock)。
2. **约束锁定** —— 求解输入的约束集被记录。
3. **结果锁定** —— 输出的精确 hash 集写入 lock。

→ 拿着同一份 lock(含索引快照 + 闭包),**无需 AI、无需重新求解**,直接重建逐字节一致的世代。AI 只在"首次求解/遇到冲突"时需要;重放历史世代纯靠 lock。

> **实证**:[`PoC-3`](../../../poc/poc3-zero-ai-solver/REPORT.md) 用纯 Python 确定性求解器 + 真实 Debian 数据验证了这一点:4 个模板(442 个包)零未解析、同输入 closure_id 三次全等、lock 可不重新求解直接重放。全程零 LLM。本节主张由代码兑现。

详见 [`../runtime/02-intent-resolved-lock.md`](../runtime/02-intent-resolved-lock.md)。

---

## 7. 与 NixOS 的对照

| 维度 | NixOS | Aevum |
|---|---|---|
| 依赖如何表达 | Nix 语言 derivation 显式描述 | 模板/约束声明 + AI 翻译意图 |
| 求解 | evaluation(求值 Nix 表达式) | 确定性约束求解器 |
| 冲突处理 | 报错,人工改表达式 | AI 提多方案 + 验证 + 兜底保留两份 |
| 可复现 | flake.lock 锁输入 | lock 锁索引快照 + 闭包 |

---

## 8. 验收清单

- [ ] 确定性求解器:同输入同输出(可复现)
- [ ] 传递闭包展开正确且闭合
- [ ] 优先复用 store 已有 hash
- [ ] 冲突检测准确
- [ ] AI 介入只改约束/意图,不直接选 hash
- [ ] foundation 包不参与让步(约束硬锁)
- [ ] closure_id 指纹稳定
- [ ] 纯 lock 重放无需 AI
