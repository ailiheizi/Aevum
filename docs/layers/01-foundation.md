# Foundation 层 —— 密封核心

> 父文档:[`README.md`](README.md)
> 关联:[`../architecture/runtime/01-generation-lifecycle.md`](../architecture/runtime/01-generation-lifecycle.md) §7、ADR [`../architecture/adr/0003-ai-maintainer-authority.md`](../architecture/adr/0003-ai-maintainer-authority.md)
> 机制原型:RELIK 的 Sealed Foundation

---

## 0. 设计哲学

> **AI 修包炸了,系统能兜底;用户手贱删了关键包,系统还能跑。
> Foundation 是那组永远在的、平台签名的、普通操作不可改的核心。**

---

## 1. 是什么

Foundation 是由平台维护并签名的**一组核心包**,构成系统启动与自愈的最小完整集。用户和 AI 的常规世代操作动不了它,只能通过"平台签名升级"专门通道变更。

### 1.1 包含哪些(建议清单)

| 包 | 作用 | 必要性 |
|---|---|---|
| `init` | 系统初始化 / 进程 1 | 没它系统起不来 |
| `aevum-maintainer` | AI 维护者运行时本体 | 没它无法自愈/求解 |
| `solver-core` | 确定性依赖求解器 | 闭包求解的引擎 |
| `store-core` | 内容寻址存储引擎 | 读写/校验 store |
| `generation-core` | 世代管理(切换/回滚) | 原子激活与回滚 |
| `min-toolset` | 最小静态工具集(shell/fs/网络) | 兜底操作,**静态链接**不依赖兼容层 |
| `linker-anchor` | 链接器锚点(可选放此或 System) | 启动期基础 |

合计约 6–8 个包,平台**永久维护这套清单**。

### 1.2 不包含哪些

- 共享运行时基线 / 系统服务 → 属 System 层。
- 用户软件、模板派生的一切 → 属 App 层。
- 任何"锦上添花"的功能 —— Foundation 只保"系统能起来 + AI 能自愈"。

> **min-toolset 为何静态链接**:兜底工具不能依赖 System 层的兼容基线,否则基线坏了兜底也跟着坏。Foundation 自包含,是最后一道防线。

---

## 2. Foundation Manifest

平台维护、随系统发布、被签名的清单:

```toml
# foundation-manifest.toml （平台维护，签名保护）
schema_version = "1.0"

[foundation]
version = "1.0.0"
sealed = true
channel = "stable"                  # stable / beta / canary
issued_at = "2026-06-08T00:00:00Z"
expires_at = "2027-06-08T00:00:00Z"

[[packages]]
name = "init"
version = "1.2.0"
hash = "sha256-..."
required = true                     # 必装，不可移除
upgrade_policy = "on-major"         # always / on-major / manual

[[packages]]
name = "aevum-maintainer"
version = "0.9.0"
hash = "sha256-..."
required = true
upgrade_policy = "always"

# ... 其余核心包

[upgrade]
check_url = "https://aevum.example/api/foundation/latest"
check_interval_hours = 24
auto_apply = false                  # 默认问用户

[signature]
algorithm = "Ed25519"
key_id = "aevum-platform-2026"
value = "..."                       # 对整个 manifest 的签名
```

### 2.1 签名链

```text
平台私钥（离线 HSM 保管，签名时人工操作）
   │ sign
   ▼
foundation-manifest.toml
   │ 随系统镜像发布
   ▼
客户端用内置 public key 验签
```

> **实现说明(第三十四轮)**:上方 `[[packages]]` 数组表头是设计期理想格式,但本项目的零依赖 TOML 解析器([`aevum_service_compiler::parse_toml_subset`],vendor 离线约束下不引 toml crate)不支持数组表头。`aevum-maintainer` 的 `FoundationManifest::parse` 实际采用**语义等价**的每包一分节形态:
>
> ```toml
> [meta]
> version = "1.0.0"
>
> [foundation.init]
> version = "1.2.0"
> required = "true"            # 解析器只认字符串,布尔写 "true"/"false"
> upgrade_policy = "on-major"
>
> [foundation.solver-core]
> version = "0.9.0"
> required = "true"
> ```
>
> `required` 缺省视为 `true`(foundation 包默认必装)。签名链(§2.1)与启动期验签(§3 步骤 2)本轮**未实现**(vendor 离线无 ed25519 crate),已落地的是 §4 的 verify 约束。

---

## 3. 启动校验

```text
boot_with_foundation():
1. 加载 foundation-manifest.toml
2. 用内置 public key 验签 → 失败则拒绝启动（防篡改）
3. 检查过期 → 过期仅告警，不阻断（避免把用户锁在门外）
4. 校验 store 中每个 foundation 包的 hash 都在且完整
5. 校验 active 世代引用了所有 required foundation 包
   └─ 缺失 → 拒绝激活该世代，转 foundation-only 兜底
```

---

## 4. 对世代操作的约束

任何 propose 出来的候选世代,verify 时强制:

```text
1. 所有 required foundation 包必须在场
   └─ 缺 → verify 失败（"不能移除核心组件: <name>"）
2. foundation 包版本必须与 manifest 精确匹配
   └─ 不符 → verify 失败（"核心包 <name> 必须是 <ver>"）
3. 求解冲突时，foundation 包的约束是硬锁，不参与让步
```

这保证:**AI 不能为了让某个 App 跑通而降级或删除核心包**。这是 AI 试错安全的地基。

---

## 5. 升级通道(唯一能动 Foundation 的路径)

```text
1. 客户端定期 GET /api/foundation/latest?channel=stable
2. 服务端返回新 manifest + 签名
3. 客户端验签 → 比较版本
4. 有新版本 → 推到 AI 询问列表：
   "🔄 系统核心可升级 v1.0 → v1.1
    内容：修复 X / 优化 Y    大小：N MB"
   [立即升级] [稍后] [跳过此版本]
5. 用户同意 → 下载新 foundation 包到 store
6. 创建新世代（新 foundation + 原有 system/app）
7. verify → activate
8. 失败 → 自动回滚到上一世代（旧 foundation 不动）
```

平台对 Foundation 的承诺:

- 永久维护清单最新版;关键修复及时推送。
- 升级向后兼容:升级后用户已有世代仍能跑。
- 升级失败可一键回退。
- 离线时已装 Foundation 永久可用。
- 不夹带广告/统计;不因商业原因下架核心包;不给个别用户偷推不同版本。

---

## 6. 极端兜底:foundation-only 模式

```text
触发：active 世代不可启动（罕见，如关键 hash 损坏）
        ↓
1. 检测到 active 起不来
2. 构造/切换到 foundation-only 世代（仅 foundation 层）
3. 系统以最小能力启动，Maintainer 在线
4. 引导用户：
   ├─ 回滚到历史 verified 世代，或
   └─ 让 Maintainer 重新求解修复
```

只要 Foundation 完整,系统就**永远能起来**。这是分层模型给用户的终极安全感。

---

## 7. AI 修复时 Foundation 是"安全锚"

```text
App 层冲突，AI 尝试修复：
  方案 A 升级冲突包 → verify
  方案 B 降级冲突包 → verify
  方案 C 保留两份   → verify
每个方案的不变约束：
  ✓ Foundation 永远不动
  ✓ AI 不能"为了让 X 跑而降级 maintainer/solver"
→ 即使所有 App/System 变更都失败，Foundation 仍能拉起最小系统。
```

详见 [`../ai/02-repair-and-keep-two.md`](../ai/02-repair-and-keep-two.md)。

---

## 8. 验收清单

- [ ] foundation-manifest schema 完整
- [ ] 平台签名 + 客户端验签跑通
- [ ] 启动期 foundation 完整性校验
- [ ] 世代操作禁止删 required foundation 包(单测)
- [ ] 世代操作禁止改 foundation 包版本(单测)
- [ ] 求解冲突不降级 foundation(行为测试)
- [ ] min-toolset 静态链接,不依赖兼容层
- [ ] 升级通道端到端 + 失败自动回滚
- [ ] foundation-only 兜底可启动
