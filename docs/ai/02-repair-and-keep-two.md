# 冲突修复与"保留两份"

> 父文档:[`README.md`](README.md)
> 关联:闭包冲突 [`../architecture/foundations/03-closure.md`](../architecture/foundations/03-closure.md) §5、store 多版本 [`../architecture/foundations/01-store.md`](../architecture/foundations/01-store.md) §5、GC [`03-garbage-collection.md`](03-garbage-collection.md)

---

## 0. 设计哲学

> **能解则解,解不了就隔离共存,绝不强行二选一炸掉用户的软件。
> "实在不行保留两份" —— 这是 Aevum 给用户的承诺。**

---

## 1. 修复何时触发

```text
- 求解器报依赖冲突（约束互斥）
- 激活后健康探针失败
- 用户装的包导致系统降级
- 安全更新引入新的版本约束冲突
```

触发后 Maintainer 进入 repair 流程。**全程不动 active**:用户当前系统照常运行,修复在世代沙盒里试。

---

## 2. 修复方案的优先级阶梯

```text
方案 A：放宽约束求单一共存版本
   ├─ AI 评估"约束是否过严"（声明 >=3.2 但实际 3.0 也够用？）
   └─ 找到一个版本同时满足各方真实需求 → 最干净
        ↓ 不行
方案 B：升级/降级一方
   ├─ 把要求老版本的一方升级到兼容新版本的版本
   └─ 或把要求新版本的一方降级（需评估功能损失）
        ↓ 不行
方案 C：保留两份，隔离共存  ←── Aevum 的兜底
   └─ 两个版本在 store 并存，各自闭包引用各自的 hash
        ↓ 连两份都装不下/相互干扰
方案 D：隔离失败 → 如实告知用户，让用户取舍
   └─ "X 和 Y 无法共存，需二选一"，绝不静默删除某一个
```

每个方案都走完整 `propose → verify`。只有 verify 通过的方案才进入候选池。

---

## 3. "保留两份"详解

### 3.1 为什么能保留两份

store 是内容寻址、多版本天然并存的(见 [`../architecture/foundations/01-store.md`](../architecture/foundations/01-store.md) §5):

```text
store/sha256-aaa/  openssl@3.2.1   ← app-X 的闭包引用
store/sha256-bbb/  openssl@3.0.13  ← app-Y 的闭包引用

两份都在磁盘上，互不知道对方存在，各跑各的。
```

### 3.2 隔离如何实现

关键在于**闭包是 per-软件的视图**:

```text
app-X 运行时，它的依赖搜索路径解析到 openssl@3.2.1 的 hash
app-Y 运行时，它的依赖搜索路径解析到 openssl@3.0.13 的 hash
→ 同一台机器，两个软件看到不同的 openssl，互不干扰
```

(具体的运行时视图隔离机制 —— 搜索路径构造 / 命名空间 —— 属实现细节,待代码阶段定稿;此处定的是语义契约:**两份并存且互不可见**。)

> **实现进度(第四十五轮)**:语义契约的**最小可验证落地**已完成。`aevum_cli::materialize_isolated_views` 为每个冲突 app 建独立依赖视图(`base/<app>`),视图里同名依赖 symlink 各指向不同版本的 store 对象;配合 `run_isolated` 的 `--library-path`,ld 按 soname 命中的即该 app 那版库。测试 `keep_two_isolation.rs` 证明两 app 各见各版本(target 指向不同 hash、内容各异)。**尚未**做的:接入世代/maintain(方案C 被采纳后自动建私有视图入世代)、两个真实冲突 Debian 包各自运行的全链路验证。

### 3.3 决策要问人

"保留两份"不是免费的(占盘、维护两份的安全更新),所以它是 `needs_user_confirm` 的关键决策:

```toml
[[decision]]
topic = "openssl 冲突"
chosen = "保留两份"
reason = "app-X 需 >=3.2，app-Y 仅兼容 3.0.x，无单一共存版本"
cost = "额外约 5MB 磁盘；两份需各自跟进安全更新"
alternatives = ["统一 3.2（app-Y 会崩）", "卸载其中一个"]
needs_user_confirm = true
```

推到 AI 询问列表,用户看清代价后拍板。

---

## 4. 修复的多候选并行

```text
触发修复
   ↓
AI 同时构造多个候选世代：
   gen-cand-A（放宽约束）   propose→verify
   gen-cand-B（升级一方）   propose→verify
   gen-cand-C（保留两份）   propose→verify
   ↓
收集所有 verify 通过的候选
   ↓
推到询问列表，附每个方案的代价/影响：
   "检测到 openssl 冲突，3 个可行方案：
    A. 都用 3.2.1（推荐，最干净）
    B. app-Y 升级到 v5（行为略变）
    C. 保留两份（+5MB）"
   ↓
用户选一个 → activate
全部失败 → active 不动，如实报告
```

> 这正是世代状态机给 AI 的试错自由:三个方案都在沙盒里验证过才给用户,选中的才激活,选错了还能秒回。

---

## 5. Foundation 永远是安全锚

修复的所有方案,有一条铁律(见 [`../layers/01-foundation.md`](../layers/01-foundation.md) §7):

```text
✓ 任何方案都不降级/删除 Foundation 包
✓ AI 不能"为了让 X 跑通而动 maintainer/solver/init"
→ 即使所有修复都失败，Foundation 仍能拉起最小系统，
  用户永远不会被卡在"连修复工具都起不来"的死局。
```

---

## 6. 修复残留与 GC

失败的候选世代不立即删除(便于审计/对比),但被标记,由 GC 按"保留失败修复 N 天"策略回收(见 [`03-garbage-collection.md`](03-garbage-collection.md))。

---

## 7. 验收清单

- [ ] 修复全程不动 active
- [ ] 方案阶梯 A→B→C→D 按优先级尝试
- [ ] 每个方案走完整 propose→verify
- [ ] "保留两份"语义:两版本并存且运行时互不可见
- [ ] "保留两份"作为 needs_user_confirm 决策,附代价
- [ ] 多候选并行验证,只推 verified 的
- [ ] 全部失败时 active 不变 + 如实报告
- [ ] 任何方案不动 Foundation
- [ ] 失败候选按策略由 GC 回收
