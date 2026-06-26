# System 层与 App 层 —— 共享与私有的边界

> 父文档:[`README.md`](README.md)
> 关联:[`01-foundation.md`](01-foundation.md)、二进制兼容 [`../architecture/runtime/03-binary-compat.md`](../architecture/runtime/03-binary-compat.md)

---

## 0. 设计哲学

> **System 是"大家共享的地基之上一层",App 是"各自的房间"。
> 共享的东西谨慎变更、可回滚;私有的东西自由折腾、互相隔离。**

---

## 1. System 层

### 1.1 职责

System 层提供"软件运行所需、但又该被共享而非各装一份"的东西:

- **共享运行时基线**:glibc、libstdc++、常见 `.so`(见 [`../architecture/runtime/03-binary-compat.md`](../architecture/runtime/03-binary-compat.md))。
- **动态链接器入口**:让普通二进制能找到 interpreter。
- **系统级服务**:显示、网络、声音等需要全局协调的服务。
- **驱动用户态垫片**:与宿主 Linux 驱动对接的用户态部分。

### 1.2 变更策略

- 由 AI 维护者管理,但**谨慎**:System 变更影响面大(很多 App 依赖它)。
- 每次 System 变更都是新世代,可回滚。
- 升级共享基线前,Maintainer 评估对现有 App 闭包的兼容性。

### 1.3 为什么不全塞进 App

如果每个 App 都自带一整套 glibc/常见库:

- 磁盘爆炸(虽然 store 去重能缓解,但运行时基线本就该统一)。
- 难以统一安全更新(一个 libssl 漏洞要改 N 份)。

所以"该共享的共享"放 System,"该私有的私有"放 App。

---

## 2. App 层

### 2.1 职责

- 用户安装的一切软件。
- 每个软件的**私有依赖**(它需要、但不该影响别人的特定版本库)。

### 2.2 自由度

- 用户/AI 自由装卸,频繁变化。
- 出冲突、出错装,**故障半径限制在 App 层**,炸不到 System/Foundation。
- AI 在这一层最敢试错(配合世代状态机,错了秒回)。

---

## 3. 共享依赖 vs 私有依赖(核心判定)

```text
某个库 libX，App-A 和 App-B 都要用：

情况 1：版本兼容（都能用 libX@2.x）
   → 放 System 基线（如果够通用）或作为共享 App 依赖，store 去重，只一份

情况 2：版本冲突（A 要 libX@1，B 要 libX@2，不兼容）
   → 各自私有：A 的闭包引用 libX@1 的 hash，
              B 的闭包引用 libX@2 的 hash
   → store 多版本并存（见 foundations/01-store.md §5）
   → 这就是"保留两份"在 App 层的体现
```

判定原则:

1. **能共享则共享**(省盘、好维护、利于统一安全更新)。
2. **冲突则私有**(隔离优先于强行统一,绝不为了"统一"而降级谁)。
3. **私有依赖绝不上浮污染 System 基线**。

---

## 4. 层间依赖规则(重申硬约束)

```text
App     ──可依赖──►  System  ──可依赖──►  Foundation
  └──────────────────可依赖──────────────────┘

反向一律禁止：
  System 不依赖 App
  Foundation 不依赖 System / App
```

后果:

- 下层永远可以在没有上层的情况下运行 → foundation-only / system 级最小集都能起来。
- 上层故障(App 崩、System 服务挂)不波及下层启动能力。
- 求解时下层约束是上层的硬边界,不被上层迁就。

---

## 5. 三层协作的一个完整例子

```text
用户："装一个需要 CUDA 的深度学习工具 DL-tool"
        ↓
Maintainer 分析：
  ├─ DL-tool 本体            → App 层
  ├─ DL-tool 私有的 python venv 依赖 → App 层（私有，不污染他人）
  ├─ CUDA 用户态运行时库     → System 层（多个 App 可能共享）评估后入基线
  ├─ 标准 C/C++ 运行时       → System 层基线（已在）
  └─ 求解器 / 世代管理        → Foundation（已在，不动）
        ↓
propose 新世代：app/ 加 DL-tool + 私有依赖；system/ 可能加 CUDA 基线
        ↓
verify（含 foundation 完整性断言）→ activate
        ↓
若 DL-tool 把自己搞坏 → 只回滚 app 层变更，system/foundation 不受影响
```

---

## 6. 验收清单

- [ ] System 基线共享、去重、世代化、可回滚
- [ ] App 私有依赖隔离,故障半径限于 App 层
- [ ] 共享/私有判定逻辑明确(兼容则共享,冲突则私有)
- [ ] 私有依赖不上浮污染 System
- [ ] 层间依赖单向向下(求解期强制)
- [ ] App 层回滚不影响 System/Foundation
