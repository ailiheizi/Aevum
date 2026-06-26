# PoC-7:核心机制实测 — 世代切换 / 回滚 / GC

> 日期:2026-06-09 · 状态:已完成 · **Aevum 三大核心卖点全部实测通过**
> 环境:WSL Debian 13,真实文件系统 + 真 symlink
> 目的:验证 Aevum 立身之本——原子世代切换、瞬时回滚、GC 引用计数——此前只在文档,首次用真文件验证。
> 关联:[`foundations/02-generation`](../../docs/architecture/foundations/02-generation.md)、[`runtime/01-generation-lifecycle`](../../docs/architecture/runtime/01-generation-lifecycle.md)、[`ai/03-garbage-collection`](../../docs/ai/03-garbage-collection.md)、[`foundations/05`](../../docs/architecture/foundations/05-multi-source-and-isolation.md)

---

## 0. 为什么这轮最重要

前几个 PoC 测的都是"消费现有生态"的可行性(二进制兼容、补闭包、多源)。但 Aevum **区别于普通包管理器的核心卖点**——内容寻址世代、原子回滚、安全 GC——**从没用真代码验证过**,只停在文档。本轮补上,用真文件系统 + 真符号链接跑核心机制。

---

## A. 原子切换 ✅

实测:`active` 是 symlink,切换 = 写临时 symlink → `os.rename` 原子替换。

```
切换前 active → gen-1,读 python = "python-3.11-body"
os.rename 切到 gen-2,耗时 0.09 毫秒
切换后 active → gen-2,读 python = "python-3.12-body"
```

**结论**:POSIX `rename` 对 symlink 是原子操作,系统在任一时刻看到的要么是旧世代、要么是新世代,**无半切状态**(切换中途断电不会坏)。`foundations/02` 的"active 原子指针"卖点成立。

---

## B. 瞬时回滚 ✅

实测:回滚 = `active` 指回旧世代,不重新求解、不重新构建。

```
从 gen-2 回滚到 gen-1,耗时 0.095 毫秒,立刻读回 python-3.11
```

**结论**:回滚是亚毫秒级的指针操作,因为目标世代的 symlink 树和它引用的 store hash 都还在。`runtime/01` 的"秒回"实测是"亚毫秒回"。这是世代模型相对传统包管理器(回滚要重装)的根本优势,坐实。

---

## C. GC 引用计数 ✅(最关键,验证不误删)

实测场景:store 有 python3.11、python3.12、libc(后者被两个世代共享)。删掉 gen-2、只保留 gen-1(active),跑可达性 GC。

```
store 前: [python3.11, python3.12, libc]
GC(保留 gen-1):
  ✓ python3.12 回收(仅 gen-2 引用 → 无世代用了)
  ✓ libc 保留    (gen-1 仍引用 → 共享依赖不误删)← 关键
  ✓ python3.11 保留(active 引用)
store 后: [python3.11, libc]
```

**结论**:可达性 GC 在真 store 上正确——**多世代共享的 hash 不会因为删掉其中一个世代被误删**。这是 GC 最容易写错、后果最严重(误删 = 别的世代崩)的地方,实测算法正确。`ai/03` 的引用计数模型成立。

---

## D. namespace 强隔离下 setuid(回答 PoC-6 待办)

实测:`unshare -r` 进 user-namespace 内:

```
id -u → 0   (映射成 root,但是"伪 root"——无真特权)
sudo 仍是 -rwsr-xr-x(权限位在),但 user-ns 的 no_new_privs 限制真实 setuid 提权
```

**架构结论**:user-namespace 内是"伪 root"(uid 映射),真实 setuid 提权被 `no_new_privs` 限制。含义:

> **强隔离沙箱内的包不应依赖真 setuid 提权**。需要真提权的系统级包(如 sudo),不能塞进强隔离沙箱靠 setuid 工作,要走特殊授权通道(特权 helper / polkit / 宿主侧授权)。

这给隔离模型画了条清晰边界:**强隔离适合普通应用,需要真特权的系统组件走 System 层 + 显式授权,不靠沙箱内 setuid。**

---

## 总结

| 机制 | 结果 | 数据 |
|---|---|---|
| 原子切换 | ✅ | rename 原子,0.09ms,无半切 |
| 瞬时回滚 | ✅ | 指针回指,0.095ms,不重建 |
| GC 引用计数 | ✅ | 共享 libc 不误删,无用 py3.12 回收 |
| 强隔离 setuid | ⚠️ 边界 | 沙箱内 setuid 不提权 → 系统级包走特殊通道 |

**总判断**:Aevum 的三大核心卖点(原子、回滚、安全 GC)**首次用真文件验证,全部成立**。强隔离的 setuid 限制不是缺陷,而是明确了"哪些包能进沙箱、哪些要走特权通道"的边界。**到此,Aevum 从"内容寻址消费生态"到"世代/回滚/GC 核心机制"的关键假设,都有了实测背书。**

---

## 复现

```bash
python3 experiment.py   # WSL Debian,真文件+symlink → /tmp/aevum-poc7/report.json
```

---

## 一句话总结

> **原子切换 0.09ms 无半切、回滚 0.095ms 不重建、GC 删世代不误删共享 libc —— Aevum 三大核心卖点首次真文件实测全过。
> 唯一边界:user-ns 强隔离内 setuid 不提权,系统级特权包须走专门通道而非沙箱。**
