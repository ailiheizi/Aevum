# 世代生命周期 —— propose / verify / activate / rollback

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:[`../foundations/02-generation.md`](../foundations/02-generation.md)、AI 维护 [`../../ai/01-maintainer-loop.md`](../../ai/01-maintainer-loop.md)

---

## 0. 设计哲学

> **变更是一个状态机,不是一条命令。每一步都可验证、可中止、可回退。**

世代状态机是 AI 维护者"敢于试错"的安全网:任何提议在激活前都被充分验证,激活后随时可秒回。

---

## 1. 状态机

```text
        propose                verify              activate
draft ──────────► candidate ──────────► verified ──────────► active
                      │                                          │
                      │ verify 失败                              │ 出问题
                      ▼                                          ▼
                   failed ──► archived              rollback（activate 上一个 verified）
                                                                 │
                                                                 ▼
                                                    旧 active 仍 verified，重新成为 active
```

| 状态 | 含义 |
|---|---|
| **draft** | 刚 propose,链接和 candidate lock 写好,尚未验证 |
| **candidate** | 完整构造完成,等待 verify |
| **verified** | 通过全部校验,可被激活 |
| **active** | 当前生效的世代(全局唯一) |
| **failed** | verify 未通过 |
| **archived** | 失败或被取代的世代,保留用于审计/GC 判定 |

---

## 2. Propose(提议变更)

```text
输入：意图（模板引用 + 声明式覆盖 + 想要的变更）
        ↓
1. AI 维护者解析意图 → 约束集
2. 确定性求解器算 closure（见 foundations/03-closure.md）
3. 检查 store 缺哪些 hash → 下载补齐并校验
4. 创建新世代目录（draft）：写 layers/ 链接 + candidate lock + manifest
5. 标记 parent = 当前 active
        ↓
输出：candidate 世代 id
```

- propose **不触碰 active**。当前系统在整个 propose 期间照常运行。
- 求解失败/冲突 → 进入 repair(见 [`../../ai/02-repair-and-keep-two.md`](../../ai/02-repair-and-keep-two.md)),可能产出多个候选。

---

## 3. Verify(验证)

候选世代必须通过全部校验才能 verified:

```text
1. 完整性：closure 内每个 hash 重算校验（见 foundations/01-store.md §4）
2. 闭合性：无未满足的 requires（闭包真的闭合）
3. Layer 约束：
   ├─ 所有 required foundation 包在场且版本精确匹配
   └─ 软件层变更未触碰 foundation（见 layers/01-foundation.md）
4. 可启动性：boot check —— 试装载关键组件，确认能起来
5. 安全与版本回退判据（防 AI 经"选签名旧版"绕过否决，见 §3.1）：
   ├─ closure 内每个包比对已知 CVE 库，命中高危 CVE → 标记，强制人工确认
   └─ 若某包版本【低于】当前 active 世代中的同名包 → 标记为"版本回退"，强制人工确认
        ↓
   全过 → verified
   任一失败 → failed → archived，active 不动
```

> verify 是 AI 试错安全的核心:错误的提议止步于此,永远到不了 active。

### 3.1 为什么需要第 5 条(回应评审 H4)

ADR-0003 边界1 限制"AI 不直接选 hash",但 AI 仍能通过**产出一个偏向旧版本的约束**(如 `==3.11.7`,而 3.11.7 含已知 CVE)来间接达成危险结果 —— 这个旧版有合法签名、能通过完整性/闭合/层校验,于是悄悄进入 verified。更危险的是:**"是否需人确认"这个判断此前由 AI 自己写**(resolved.decision 的 `needs_user_confirm`),AI 误标/恶意标记为"无需确认"就能绕过人类否决,形成循环论证。

第 5 条把这个判断从"AI 自述"改为"**verify 阶段的确定性、独立检查**":

- CVE 命中与版本回退由 verify **机器判定**,不信任 AI 的 `needs_user_confirm` 自述。
- 命中即**强制**进入人工确认路径,AI 无权把它标成"自动通过"。
- 这切断了"AI 既提议、又自评是否危险、还自己放行"的闭环 —— 危险判定独立于提议者。

---

## 4. Activate(激活)

```text
原子操作序列：
1. 写 active 指针 → 新世代（原子 rename / 原子写）
2. 更新根 lock 镜像（指向新世代的 lock）
3. 通知运行中的实例热重载
4. 旧 active 世代保留（成为回滚目标）
```

- 第 1 步是**唯一的真正切换点**,原子完成,无中间态。
- 前后系统要么完全是旧世代、要么完全是新世代。

---

## 5. Rollback(回滚)

```text
1. 从 parent 链 / verified 列表选目标（默认 parent）
2. 直接 activate 该世代 —— 用它的历史 lock，不重新求解
3. < 1 秒完成
4. 被回滚掉的世代不销毁，仍可前滚
```

回滚之所以能秒级且安全:目标世代的 lock 和它引用的 store hash 都还在,内容逐字节一致。这是 store 不可变 + 世代不可变两条不变量的直接红利。

---

## 6. AI 修复时的状态机用法

AI 维护者遇到依赖问题时,把状态机当沙盒:

```text
问题：app 层某包冲突导致系统降级
        ↓
1. active 不动（用户仍在当前状态）
2. AI 生成多个候选世代：
   方案A propose→verify   方案B propose→verify   方案C(保留两份)propose→verify
3. 只把 verified 的方案推给用户/询问列表
4. 用户选一个 → activate
5. 若全失败 → active 保持不变（停在原始问题态，但系统仍可用）
```

详见 [`../../ai/02-repair-and-keep-two.md`](../../ai/02-repair-and-keep-two.md)。

---

## 7. 极端兜底:foundation-only 模式

```text
若 active 世代不可用（罕见，例如磁盘损坏波及关键 hash）：
1. 客户端检测到 active 无法启动
2. 自动构造/切换到 foundation-only 世代（仅含 foundation 层）
3. 系统以最小能力启动，AI 维护者在线
4. 引导用户从历史 verified 世代恢复或重新求解
```

详见 [`../../layers/01-foundation.md`](../../layers/01-foundation.md) §极端兜底。

---

## 8. 不变量

1. propose / verify 期间 active 绝不改变。
2. 只有 verified 世代能被 activate。
3. activate 的切换点是单个原子操作。
4. rollback 不重新求解,只切指针。
5. 任何时刻系统总能落到一个可启动世代(最坏是 foundation-only)。

---

## 9. 验收清单

- [ ] 五状态流转完整,非法转移被拒
- [ ] propose 不触碰 active(并发验证)
- [ ] verify 五类校验齐全(完整性/闭合/层约束/可启动/安全回退)
- [ ] CVE 命中与版本回退由 verify 机器判定,不信任 AI 的 needs_user_confirm 自述
- [ ] activate 原子切换,无中间态
- [ ] rollback < 1 秒,可前滚
- [ ] 修复全失败时 active 不变
- [ ] foundation-only 兜底可启动
