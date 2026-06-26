# 三文件层 —— Intent / Resolved / Lock

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:[`../foundations/03-closure.md`](../foundations/03-closure.md)、[`01-generation-lifecycle.md`](01-generation-lifecycle.md)、ADR [`../adr/0002-no-dsl-intent-layer.md`](../adr/0002-no-dsl-intent-layer.md)

---

## 0. 设计哲学

> **意图、求解、锁定分成三层文件。
> 人只碰意图层(纯数据,无 DSL);AI 产出求解层;系统生成锁定层。
> 锁定层保证逐字节可复现。**

这借鉴 NixOS 的"声明 / 配置 / lock"三段式,但关键差异是:**意图层不是 Nix 语言,是纯声明式数据**。

---

## 1. 三层一览

| 层 | 文件 | 是什么 | 谁写 | 可复现角色 |
|---|---|---|---|---|
| **Intent** | `intent.toml` | 用户意图:模板引用 + 声明式覆盖 | 人 / AI | 表达"想要什么" |
| **Resolved** | `resolved.toml` | 求解后的具体计划:选定版本、来源、约束推导 | AI 维护者 | 中间态,可审计 |
| **Lock** | `lock.toml` | 锁定快照:精确 hash 闭包 + 索引快照 | 系统生成 | 保证"得到的完全一致" |

```text
intent.toml  ──AI 解析──►  resolved.toml  ──求解器+构建──►  lock.toml
 "我想要"                    "打算这么装"                     "锁死成这样"
 人可读可改                  人可审，AI 可改                  机器权威，不手改
```

---

## 2. Intent 层(人面对的唯一一层)

纯数据,没有函数、没有惰性求值、没有图灵完备性。一个会读 TOML 的人就能看懂改动。

```toml
# intent.toml
schema_version = "1.0"

[system]
template = "minimal-desktop"      # 基础蓝图

[[use]]
template = "dev-python-ds"        # 叠加：数据科学环境

[overrides]
# 声明式覆盖，不是代码
"python".version = "3.11"         # 我要 3.11，不要默认的 3.12
"postgresql".enable = true

[exclude]
packages = ["telemetry-agent"]    # 我不要这个
```

> 对比 NixOS:这里没有 `let ... in`、没有 `pkgs.lib.mkIf`、没有 overlay 函数。意图层故意保持"配置而非编程"。需要逻辑的地方(条件、组合、求解)交给 AI 维护者和模板系统,而不是逼用户写 DSL。理由见 [`../adr/0002-no-dsl-intent-layer.md`](../adr/0002-no-dsl-intent-layer.md)。

---

## 3. Resolved 层(AI 的工作产物,人可审计)

AI 维护者把 intent 翻译成明确计划:展开模板、推导约束、初步选版本。这一层让 AI 的决策**可见、可审计、可否决**,而不是黑盒。

```toml
# resolved.toml （由 AI 维护者生成）
schema_version = "1.0"
resolved_at = "2026-06-08T12:00:00Z"
intent_digest = "sha256-..."      # 对应的 intent 指纹

[[package]]
name = "python"
version = "3.11.8"
reason = "intent.overrides 指定 3.11；3.11.8 是该线最新补丁"
source = "aevum-index:python@3.11.8"

[[package]]
name = "openblas"
version = "0.3.26"
reason = "numpy@1.26.4 的传递依赖"
source = "aevum-index:openblas@0.3.26"

[[decision]]
# AI 做的关键取舍，供人类审查/否决
topic = "openssl 版本冲突"
chosen = "保留两份：app-X→3.2.1, app-Y→3.0.13"
alternatives = ["统一升级到 3.2.1（Y 不兼容，否决）"]
```

`reason` / `decision` 字段是 AI 维护者透明性的载体 —— 人能看懂 AI 为什么这么选,并在关键决策上否决。

---

## 4. Lock 层(机器权威,可复现的命脉)

锁定一切,使世代可逐字节重建。**不手改**。

```toml
# lock.toml
schema_version = "1.0"
locked_at = "2026-06-08T12:00:05Z"
closure_id = "clo-9f86d081"
intent_digest = "sha256-..."
resolved_digest = "sha256-..."

[index_snapshot]
# 求解时所用的索引状态被钉死——这样重放无需联网、无需重新求解
id = "idx-2026-06-08"
digest = "sha256-..."

[[locked]]
name = "python"
version = "3.11.8"
hash = "sha256-3a7bd3e2..."        # 精确到 store hash
layer = "app"

[[locked]]
name = "openblas"
version = "0.3.26"
hash = "sha256-..."
layer = "app"

# ... 闭包内每一个包
```

### 4.1 可复现的三重锁定

1. `index_snapshot` —— 锁住"当时有哪些包可选"。
2. `intent_digest` / `resolved_digest` —— 锁住意图与计划。
3. 每个 `[[locked]]` 的 `hash` —— 锁住具体内容。

→ **拿着 lock.toml,在任何机器上,无需 AI、无需联网重解,重建出逐字节一致的世代。** AI 只在首次求解或遇到冲突时参与;历史世代的重放是纯确定性的。

---

## 5. 三层与世代的关系

```text
每个世代 gen-N/ 都内嵌一份 lock.toml（该世代的权威快照）。
intent.toml / resolved.toml 是"通往新世代的过程产物"，
而 lock.toml 是"世代本身的身份证"。

回滚 = 拿历史世代的 lock.toml 直接重建/激活，跳过 intent 和 resolved。
```

---

## 6. 与 NixOS 的对照

| | NixOS | Aevum |
|---|---|---|
| 声明层 | `configuration.nix` / `flake.nix`(**Nix 语言,图灵完备**) | `intent.toml`(**纯数据,无 DSL**) |
| 求解层 | evaluation(隐式,在 nix 内) | `resolved.toml`(**显式、可审计**) |
| 锁定层 | `flake.lock`(锁 flake 输入) | `lock.toml`(锁索引快照 + 完整闭包 hash) |
| 谁处理逻辑 | 用户写 Nix 表达式 | AI 维护者 + 模板 |

---

## 7. 验收清单

- [ ] intent.toml 纯数据,无可执行逻辑
- [ ] resolved.toml 带 reason/decision,AI 决策可审计
- [ ] lock.toml 三重锁定齐全(索引快照 + 摘要 + 逐包 hash)
- [ ] 纯 lock 重放:无 AI、无联网,逐字节一致
- [ ] 世代内嵌 lock 作为身份
- [ ] 回滚跳过 intent/resolved,直接用历史 lock
