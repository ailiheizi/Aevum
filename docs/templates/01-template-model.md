# 模板数据模型

> 父文档:[`README.md`](README.md)
> 关联:意图层 [`../architecture/runtime/02-intent-resolved-lock.md`](../architecture/runtime/02-intent-resolved-lock.md)

---

## 0. 目的

把模板的数据结构、解析顺序、合并/覆盖语义钉死,供未来实现遵循。

---

## 1. 模板文件结构

> **实现说明(第五十一轮)**:本项目零依赖 TOML 解析器 `parse_toml_subset` 不支持 `[[array-of-tables]]`,
> 故 `crates/template` 实现采用**分节形态**(语义等价、解析器原生支持),跟随 foundation manifest 先例:
> `[[capability]]` → `[capability.<id>]`,`[[optional]]` → `[optional.<id>]`;布尔写字符串 `"true"`/`"false"`。
> 下方设计原稿(`[[capability]]`)保留作意图参考,实际模板文件见 `templates/*.toml`。

```toml
# templates/<name>.toml
schema_version = "1.0"

[template]
name = "dev-python-ds"
title = "Python 数据科学环境"
description = "python3 + numpy/pandas/jupyter + BLAS 加速"
version = "1.0.0"                  # 模板自身版本（变更要可追踪）
extends = []                      # 继承的父模板（可多个）

# ── 该模板声明的高层能力意图 ──
[[capability]]
id = "python3"
constraint = ">=3.10"             # 版本约束（声明，不指定 hash）
layer_hint = "app"                # 建议落在哪一层（最终由 Maintainer 定）

[[capability]]
id = "numpy"
constraint = ">=1.26"
layer_hint = "app"

[[capability]]
id = "blas-runtime"
constraint = "*"
layer_hint = "system"             # 可能共享 → 提示 System 层

# ── 可选组件（用户可在 override 中开关）──
[[optional]]
id = "jupyter"
default = true

[[optional]]
id = "cuda-runtime"
default = false                   # 默认不开，需要 GPU 的人自己开
```

---

## 2. 关键字段语义

| 字段 | 含义 |
|---|---|
| `extends` | 继承的父模板列表;先展开父,再叠加本模板 |
| `capability.id` | 抽象能力标识(对应 store 包的 `provides.capabilities`) |
| `capability.constraint` | 版本约束(semver 风格),交给求解器 |
| `capability.layer_hint` | 层归属建议;Maintainer 可据实际调整,但不能把意图塞进 Foundation |
| `optional` | 可选组件 + 默认开关;用户 override 可改 |

> 注意:模板**只给约束,不给 hash**。把"装哪个具体 hash"留给求解器,是可复现与灵活性的分界 —— 模板表达"想要",lock 记录"得到"。

---

## 3. 解析与合并顺序

```text
1. 展开 extends（递归，深度优先）
   templates A extends [B, C]：先得到 B 的能力集、C 的能力集
2. 按声明顺序合并能力
   后者覆盖前者的同 id 约束（更具体的胜出）
3. 叠加 intent.toml 里的 [[use]] 多模板
   同样按顺序合并
4. 应用 intent.toml 的 [overrides] / [exclude]
   用户意图最高优先级
5. 输出：合并后的"约束集" → 交给确定性求解器
```

冲突处理:同一能力出现互斥约束时,**用户 override > 后声明的模板 > 先声明的模板**;若仍无法满足,进入 AI 维护者的 repair 流程(见 [`../ai/02-repair-and-keep-two.md`](../ai/02-repair-and-keep-two.md))。

---

## 4. 合并示例

```text
minimal-desktop:   {browser>=1.0}
dev-rust:          {rustc>=1.75, cargo>=1.75}
dev-python-ds:     {python3>=3.10, numpy>=1.26, blas-runtime}
intent.overrides:  {python3 = "3.11"}      # 锁死小版本
intent.exclude:    {browser}               # 不要浏览器

合并结果约束集：
  rustc>=1.75, cargo>=1.75,
  python3 ==3.11.*, numpy>=1.26, blas-runtime
  （browser 被排除）
        ↓ 求解器
  精确闭包 → lock
```

---

## 5. 模板版本与可追踪

- 模板自身有 `version`,变更要 bump,便于追溯"这个世代是用哪版模板生成的"。
- 世代 manifest 记录所用模板及其版本(见 [`../architecture/foundations/02-generation.md`](../architecture/foundations/02-generation.md) §5)。
- 派生模板的 `extends` 形成依赖图,平台/用户可校验无环。

---

## 6. 与 Foundation 的硬边界

```text
模板能声明 layer_hint = "app" / "system"
模板【不能】声明 layer_hint = "foundation"
模板【不能】引用任何 foundation 包的增删改
→ 解析期校验：发现模板试图触碰 foundation → 直接拒绝该模板
```

理由见 [`../layers/01-foundation.md`](../layers/01-foundation.md) 与 [`../architecture/adr/0003-ai-maintainer-authority.md`](../architecture/adr/0003-ai-maintainer-authority.md)。

---

## 7. 验收清单

- [ ] 模板 schema 完整(template/capability/optional)
- [ ] extends 递归展开,无环校验
- [ ] 合并优先级正确(override > 后模板 > 前模板)
- [ ] optional 默认开关 + override 可改
- [ ] 模板只给约束不给 hash
- [ ] layer_hint 不可为 foundation,违者拒绝
- [ ] 世代记录所用模板及版本
