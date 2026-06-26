# AI 增强层实现设计 —— IntentResolver 与确定性核心的边界

> 父文档:[`README.md`](README.md)
> 关联:ADR [`../architecture/adr/0003-ai-maintainer-authority.md`](../architecture/adr/0003-ai-maintainer-authority.md)(权限边界)、ADR [`../architecture/adr/0005-ai-model-form-factor.md`](../architecture/adr/0005-ai-model-form-factor.md)(模型形态)、维护循环 [`01-maintainer-loop.md`](01-maintainer-loop.md)
> 状态:**实现设计(待实现)** · 日期:2026-06-11
> 前提:确定性核心(solver)已实现并在真实 Debian 索引端到端验证(见 [`../CHANGELOG.md`](../CHANGELOG.md) 第十六轮、里程碑5)

---

## 0. 这篇文档解决什么

ADR-0003/0005 定了 AI 的**权限边界**与**模型形态**,`01-maintainer-loop` 描了**工作循环**。但都未落到"AI 增强层在 Rust 实现里长什么样、与已实现的 solver 怎么接"。本文档补这一层实现设计。

**一句话**:AI 增强层是 solver 的**前置适配器**——把模糊意图翻译成 solver 已能吃的 `Vec<Constraint>`,不碰任何已验证的确定性逻辑。

---

## 1. 为什么这层是"适配器"而非"改造"

里程碑5 已验证的确定性核心(`aevum_solver`):

```text
Vec<Constraint>(顶层包名/约束) → resolve() → build_lock() → closure_id(可复现)
   coreutils 真实例:coreutils → 15 包闭包 → clo-58e5cd36684a7802(跨平台一致)
```

AI 增强层加在**最前面**,产出物正是 solver 的**现有输入**:

```text
"我要数据科学环境"
      │  IntentResolver(AI 增强,可选)
      ▼
{python3>=3.10, numpy>=1.26, ...}  ← Vec<Constraint>,solver 已能吃
      │  aevum_solver::resolve(已实现,确定性,零改动)
      ▼
精确闭包 + closure_id(可复现,无 AI)
```

→ **关键性质**:AI 的非确定性被隔离在"产约束"阶段(ADR-0003 边界1)。solver 之后全程零 AI,可复现性不受影响。已实现的 solver/store/generation **一行不改**。

---

## 2. 核心抽象:`IntentResolver` trait

新增 crate `intent`(或 solver 内子模块),定义:

```rust
/// 把模糊意图翻译成确定性求解器能吃的约束集(AI 增强层,ADR-0003 边界1)。
pub trait IntentResolver {
    /// 输入意图,输出约束集 + 翻译过程的可审计记录。
    /// 失败(模型不可用/无法翻译)返回 Err,调用方降级到"要求显式约束"。
    fn resolve_intent(&self, intent: &Intent) -> Result<IntentOutcome, IntentError>;
}

pub enum Intent {
    /// 自然语言:"帮我把开发环境换成 Python 3.11"
    NaturalLanguage(String),
    /// 模板选择:dev-python-ds(模板→约束,无需 AI,确定性)
    Template(String),
    /// 直接约束(用户已写好,IntentResolver 透传,无需 AI)
    Explicit(Vec<Constraint>),
}

pub struct IntentOutcome {
    /// 翻译出的约束集 —— 喂给 aevum_solver::resolve。
    pub constraints: Vec<Constraint>,
    /// 可审计记录(ADR-0003 边界3:写进 lock 的 ai_assist)。
    pub assist: AiAssist,
}

pub struct AiAssist {
    /// 本次是否有 AI 介入(Template/Explicit = false,NaturalLanguage = true)。
    pub ai_involved: bool,
    /// 模型标识(本地/云端/none),ADR-0005 lock 记录用。
    pub model_id: Option<String>,
    /// AI 产出的约束(供审计:为什么是这些约束)。
    pub reason: String,
    /// 需用户确认的关键取舍(ADR-0003 边界3 询问列表)。
    pub needs_user_confirm: Vec<String>,
}
```

### 三种 Intent 的处理(只有一种需要 AI)

| Intent | 是否需 AI | 处理 |
|---|---|---|
| `Explicit(constraints)` | 否 | 透传——这正是里程碑5 cli `resolve` 现在做的 |
| `Template(name)` | 否 | 读模板 TOML → 约束集(确定性,见 `templates/`) |
| `NaturalLanguage(s)` | **是** | 调模型翻译;模型不可用则 Err → 降级 |

→ **印证 ADR-0005**:确定性核心(Explicit/Template)离线永久可用;AI 增强(NaturalLanguage)可选、离线降级。

---

## 3. 多形态实现(ADR-0005:模型可插拔)

`IntentResolver` 的实现按部署形态分,**全部产出同样的 `Vec<Constraint>`**:

```rust
/// 1. 确定性 mock:关键词/规则映射,不调 LLM。
///    用途:测试、离线、CI;验证全链路而不依赖网络/模型。
pub struct MockIntentResolver { rules: HashMap<String, Vec<Constraint>> }

/// 2. 本地模型:调本地 LLM(默认优先,ADR-0005)。
pub struct LocalModelResolver { /* 模型句柄 */ }

/// 3. 云端/自带 key:调远端 API。
pub struct RemoteModelResolver { /* endpoint, key */ }
```

实现顺序建议:**先 MockIntentResolver**(确定性、可测、不引网络依赖,与项目"vendor 离线"约束一致),把"意图→约束→solver→闭包"全链路在测试里跑通;真模型(本地/云端)作为后续可插拔实现,接入时不改 trait、不改 solver。

---

## 4. lock 的 `ai_assist` 字段(ADR-0005 要求)

cli `resolve` 编排扩展:把 `IntentOutcome.assist` 写进 lock(纯文本,不引 serde_json,与里程碑5 一致):

```text
closure_id: clo-58e5cd36684a7802
package_count: 15
unresolved: 0
ai_assist: involved=false model=none           # Explicit/Template
# 或: ai_assist: involved=true model=local:qwen reason="数据科学→python3+numpy+pandas"
---
coreutils@9.4-1#sha256:...
...
```

**关键(ADR-0005 §3)**:lock 记录 AI **是否参与及如何参与**(审计需求),但**重放只用确定性部分**(closure + index 快照),不重跑模型。即模型不可用,历史世代照样逐字节重建。`ai_assist` 是审计信息,不是重放依赖。

---

## 5. 与已实现代码的接点(最小改动清单)

| 组件 | 改动 | 性质 |
|---|---|---|
| `aevum_solver` | **零改动** | 已能吃 `Vec<Constraint>`(里程碑5) |
| 新增 `intent` crate | `IntentResolver` trait + `MockIntentResolver` + `Intent`/`IntentOutcome` | 新增,不依赖 store/generation |
| `cli::resolve` | 接 IntentResolver:`Intent → IntentOutcome → 现有 solver 路径` | 扩展,向后兼容(Explicit 等价现状) |
| `cli` lock 写入 | 加 `ai_assist` 行 | 扩展纯文本格式 |
| 模板 crate(未实现) | Template→约束(确定性) | 可与 AI 层独立推进 |

→ AI 层完全不碰 Foundation/store/generation/elf/closure-builder(ADR-0003 边界2:不动 Foundation 的实现体现)。

---

## 6. 测试策略(延续项目"真实验证"调性)

- **跨平台单测**:MockIntentResolver 规则映射、Intent 三态分发、ai_assist 记录。
- **端到端(真实数据)**:`NaturalLanguage("数据科学环境")` → Mock 翻译 → 真实 Debian 索引求解 → closure_id 可复现 + lock 含 ai_assist。复用里程碑5 的真实 6.8 万包索引。
- **离线降级**:模型不可用时 NaturalLanguage 返回 Err,Explicit/Template 仍 Ok(验证 ADR-0005 降级)。
- **可复现不依赖 AI**:同意图两次求解,只要约束集相同 → closure_id 相同;换 Mock 规则但产出同约束 → 仍同 closure_id(证明可复现来自约束/lock,非模型)。

---

## 7. 诚实标注的边界

- **真模型接入是可选后续**:MockIntentResolver 先验证机制与全链路;真 LLM(本地/云端)接入需处理网络/模型依赖,与项目"vendor 离线、不引依赖"约束有张力,应作为独立可插拔实现单独评估。
- **本文档是实现设计,非实现**:遵循项目"设计→实证→实现"方法论;实现前应如里程碑1-5 一样,先确认可验证、有数据。
- **不扩大 AI 权限**:本设计严格落在 ADR-0003 三边界内——AI 只产约束(边界1)、不碰 Foundation 实现(边界2)、决策写 ai_assist 供否决(边界3)。不新增任何 AI 对系统的直接操作权。

---

## 8. 一句话总结

> AI 增强层 = solver 的前置意图适配器。已实现的确定性核心(里程碑5)是地基且零改动;
> AI 把"人话"翻译成 solver 已能吃的约束,翻译过程记进 lock 供审计,但可复现永远只来自确定性的 closure + lock,不来自模型。
