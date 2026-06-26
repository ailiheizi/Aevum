# PoC-3:零 LLM 确定性求解器 — 实测报告

> 日期:2026-06-08 · 状态:已完成 · 全部验证通过(all_pass: true)
> 目的:证明 Aevum 核心路径「模板 + override → 求解闭包 → 产 lock」**完全不调 LLM** 也能跑通且可复现。
> 回应:评审 H5(AI 模型形态)、H6(确认疲劳),并用代码兑现 ADR-0003 边界1 与 ADR-0004 可复现性主张。
> 数据:真实 Debian stable Packages(PoC-1 已下载,68,755 包)。

---

## 0. 为什么做这个 PoC

评审对"AI 作为第一维护者"提了两个 HIGH:

- **H5**:AI 模型本体没设计,lock 锁了一切唯独不锁"由哪个模型求解",与"离线 Foundation 永久可用"冲突。
- **H6**:依赖 AI 在场是新门槛(离线、隐私、成本),确认疲劳会把人类否决权磨成橡皮图章。

两者的根都是一个问题:**装个软件到底离不离得开 AI?** 如果离不开,Aevum 就有了 NixOS 没有的新依赖。ADR-0003 边界1 和 ADR-0004 都声称"AI 只产意图,确定性求解器算 hash,可复现只来自 lock"——本 PoC 用代码检验这句话是不是真的。

---

## 1. 做了什么

纯 Python 实现一个确定性依赖求解器(`solver.py`,~280 行,零外部依赖、零网络、零 AI):

- 读真实 Debian `Packages` 建依赖图(name → 版本列表、Depends、Provides、SHA256)。
- 输入:模板(顶层包意图)+ override(pin 版本 / exclude)。
- 做传递闭包展开 + 版本约束求解 + 虚包/alternatives 解析。
- 输出 lock:闭包内每个包的 `name@version` + 真实内容指纹 + `closure_id`。

**确定性的四个保证**(代码层面):
1. 版本选择规则固定:满足约束者中选版本号最大(Debian 版本比较算法)。
2. 闭包展开用按包名排序的工作队列(无遍历顺序歧义)。
3. alternatives(`a|b`)固定取第一个索引中存在的。
4. `closure_id` = 对排序后的 `(name,version,fingerprint)` 取 SHA-256。

→ 无 `random`、无时钟、无 AI 调用。同输入 + 同索引快照 ⇒ 同输出。

---

## 2. 结果

### 2.1 四个模板全部解通(真实 Debian 数据)

| 模板 | 顶层意图 | 闭包包数 | 未解析 | alternatives 处理 |
|---|---|---|---|---|
| `dev-python` | python3 + pip | 40 | **0** | 4 |
| `cli-tools` | ripgrep + jq + git | 52 | **0** | 0 |
| `web-server` | nginx-core + curl | 99 | **0** | 5(+1 虚包) |
| `media` | ffmpeg + imagemagick | **251** | **0** | 11 |

`media` 251 个包(ffmpeg 的完整传递依赖)纯算法解通、零未解析,说明这套确定性求解在真实规模上成立。

### 2.2 六项验证全部通过

| 验证 | 含义 | 结果 |
|---|---|---|
| `determinism_same_input_3x` | 同输入连解 3 次 closure_id 全等 | ✅ `clo-52afec...` ×3 |
| `distinct_templates_distinct_ids` | 不同模板产出不同 closure_id | ✅ |
| `override_exclude_changes_closure` | 排除 curl,99→80 包,id 改变 | ✅ |
| `lock_replay_without_solving` | 从 lock 重算 closure_id 一致,**不重新求解** | ✅ |
| `real_content_addressed_fingerprints` | 52/52 包带真实 Debian SHA256 | ✅ |
| (隐含)`unresolved == 0` | 4 模板共 442 包闭包零未解析 | ✅ |

`all_pass: true`。

---

## 3. 结论:AI 是可选增强,不是必需门槛

### 3.1 直接裁决

- **"装软件离不离得开 AI"——答案:离得开。** 模板→求解→lock 这条核心路径用纯算法跑通了真实 Debian 数据,确定、可复现、可重放。
- **ADR-0003 边界1 被代码证实**:确定性求解器独立算出 hash 闭包,不需要 AI 选包。
- **ADR-0004 可复现性主张被代码证实**:`lock_replay_without_solving` 通过——拿着 lock 不重新求解、不调 AI 就能还原闭包身份。历史世代重放是纯确定性的。

### 3.2 对 H5/H6 的回应

| 评审问题 | PoC-3 的回应 |
|---|---|
| **H5** AI 模型形态、离线冲突 | AI **不在**求解/重放热路径。离线时:确定性求解器照常工作,历史 lock 照常重放。AI 模型形态问题被降级——它影响的是"首次把模糊意图翻译成模板/约束"的便利性,不影响"系统能不能装、能不能复现"。 |
| **H6** 依赖 AI 是门槛 / 确认疲劳 | 用 TOML 模板 + override 即可完成全流程,**一次 AI 都不用调**。AI 从"必经门槛"变成"想用自然语言时的可选入口"。确认疲劳的根因(凡事问 AI)被釜底抽薪。 |

### 3.3 这如何重新定位 AI 在 Aevum 的角色

```text
此前隐含的(被评审质疑的)：AI 在装软件的热路径上 → 离线/成本/信任都成问题
PoC-3 证实的边界：
   AI 的位置 = 意图层「翻译」+ 冲突修复「出主意」（离线热路径之外、可选）
   求解/闭包/lock/重放/回滚 = 纯确定性，零 AI
→ 没有 AI，Aevum 退化为"一个可复现、原子、分层的包管理器"——仍然可用，只是少了自然语言便利和自动修复。
   有 AI，它在确定性骨架之上提供智能增强。
```

这正是"智能而不失控"该有的样子,也让 Aevum 的离线/隐私/成本叙事站得住。

---

## 4. 诚实声明(本 PoC 的边界)

- 这是**可行性原型**,不是生产求解器。Debian 版本比较算法用了简化版(够确定、够覆盖测试,但未通过完整 dpkg 版本规范测试集)。
- 版本冲突的"保留两份"分支未实现(本 PoC 的 4 个模板未触发硬冲突);它是 closure 求解失败后的处理,不影响"零 AI 能求解"的结论。
- alternatives 用"取第一个存在的"确定性规则,真实场景可能需要更聪明的策略——但那恰好是 AI 可选增强的发力点,且不破坏确定性(AI 产约束,求解器仍确定)。
- 内容指纹直接用 Debian 索引的 SHA256;Aevum 自己的内容寻址规范化(见 store 文档)是另一层,本 PoC 未重算。

---

## 5. 复现方式

```bash
cd poc/poc3-zero-ai-solver
python solver.py cli-tools                    # 单模板求解 → out/lock-cli-tools.json
python solver.py web-server --exclude curl    # 带 override
python verify.py                              # 六项验证 → out/verify_result.json
```

依赖 PoC-1 已下载的 `poc1-index-feasibility/data/Packages.gz`。纯 Python,无第三方库。

---

## 6. 一句话总结

> **模板→求解→lock→重放,全程零 LLM,4 个模板 442 个真实 Debian 包零未解析,同输入 closure_id 三次全等,lock 可不求解重放。
> 证明 AI 在 Aevum 是确定性骨架之上的可选增强,不是装软件的必需门槛——H5/H6 的根被拔掉,ADR-0003/0004 由代码兑现。**
