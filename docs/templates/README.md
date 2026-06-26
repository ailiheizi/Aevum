# 模板系统

> 父文档:[`../README.md`](../README.md)
> 子文档:[`01-template-model.md`](01-template-model.md)
> 关联:意图层 [`../architecture/runtime/02-intent-resolved-lock.md`](../architecture/runtime/02-intent-resolved-lock.md)、求解 [`../architecture/foundations/03-closure.md`](../architecture/foundations/03-closure.md)

---

## 0. 设计哲学

> **模板是蓝图,不是脚本。
> 你说"我要个 Rust 开发环境",选一个模板;它带来一组高层意图,AI 求解成具体世代。**

模板系统是 Aevum "无 DSL 也能组织复杂系统"的答案:NixOS 用 module/overlay(写 Nix 代码)做组合,Aevum 用模板(声明式数据 + AI 求解)做组合。

---

## 1. 模板是什么

一份声明式的"系统/环境意图蓝图":

- 它声明"这类系统/环境**想要什么能力**",而不是"具体装哪个 hash"。
- 选用一个模板 = 给 Maintainer 一组高层意图,由确定性求解器算成精确闭包。
- 模板**可组合、可派生、可覆盖**。

```text
模板（蓝图，声明能力）
   ↓ 选用 + 覆盖
intent.toml（用户意图）
   ↓ AI 解析 + 求解
closure（精确 hash）
   ↓ propose/verify/activate
Generation（世代）
```

---

## 2. 内置模板(示例)

| 模板 | 用途 | 带来的高层意图 |
|---|---|---|
| `minimal-desktop` | 最小桌面 | 显示、输入法、文件管理器、浏览器 |
| `dev-rust` | Rust 开发 | rustc、cargo、常见构建依赖、LSP |
| `dev-python-ds` | Python 数据科学 | python3、numpy、pandas、jupyter、BLAS |
| `dev-web` | 前端开发 | node、包管理器、浏览器、调试工具 |
| `server-web` | Web 服务器 | 反向代理、运行时、证书管理 |
| `server-db` | 数据库服务器 | 数据库、备份工具、监控 |

> 模板只声明意图,具体版本由求解器在当前索引快照内选定并锁进 lock,所以同一模板在不同时间求解可能得到不同补丁版本 —— 但一旦锁定就完全可复现。

---

## 3. 组合与派生

### 3.1 叠加多个模板

```toml
# intent.toml
[system]
template = "minimal-desktop"      # 基底

[[use]]
template = "dev-rust"             # 叠加 Rust 开发

[[use]]
template = "dev-python-ds"        # 再叠加数据科学
```

Maintainer 把多个模板的意图**合并**,统一求解一个闭包(共享依赖自动去重)。

### 3.2 派生自定义模板

```toml
# templates/my-fullstack.toml
schema_version = "1.0"

[template]
name = "my-fullstack"
extends = ["dev-rust", "dev-web", "server-db"]   # 继承三个

[adds]
# 在继承之上追加意图
capabilities = ["docker-cli", "redis-client"]

[overrides]
"node".version = "20"             # 锁定 node 大版本
```

派生模板让团队/个人把"我们项目的标准环境"固化成一个可复用蓝图,新人选它即可得到一致环境。

---

## 4. 覆盖(override)

模板给默认,用户覆盖细节 —— 都是声明式,不写代码:

```toml
[overrides]
"python".version = "3.11"         # 覆盖版本
"postgresql".enable = true        # 开启可选组件
"telemetry".enable = false        # 关闭某项

[exclude]
packages = ["heavy-ide-plugin"]   # 排除模板默认带的某个包
```

覆盖与排除被记录在 intent,经 resolved 体现 AI 如何采纳,最终锁进 lock。

---

## 5. 模板与分层的关系

模板的意图会被 Maintainer **分配到合适的层**:

```text
dev-python-ds 模板：
  ├─ python / numpy / pandas        → App 层（开发环境，私有）
  ├─ BLAS 运行时（若多环境共享）     → System 层评估
  └─ 绝不触碰 Foundation
```

模板**永远不能**声明要改 Foundation 包 —— 那是签名升级通道的专属(见 [`../layers/01-foundation.md`](../layers/01-foundation.md))。

---

## 6. 模板与可复现

- 模板本身是声明式数据,可纳入版本管理、可分享。
- 模板 + 索引快照 → 确定的 closure → 确定的世代。
- 分享一个派生模板 = 分享一套可复现环境的"配方";对方求解后用 lock 锁定,得到一致结果。

> 对比 NixOS:分享 `flake.nix` 要求对方能读懂 Nix 表达式;分享 Aevum 模板只是一份 TOML 蓝图,门槛低得多。

---

## 7. 验收清单

- [ ] 内置模板集合定义清晰
- [ ] 多模板叠加,意图合并 + 依赖去重
- [ ] 派生模板(extends/adds/overrides)
- [ ] 声明式覆盖与排除生效
- [ ] 模板意图正确分配到 Foundation/System/App 层
- [ ] 模板不能声明改 Foundation
- [ ] 模板可分享且求解可复现
