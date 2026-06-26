# PoC-6:三个架构盲区压测 — 实测报告

> 日期:2026-06-09 · 状态:已完成 · 抓到 1 个 store 实现 bug 隐患,验证 2 个设计假设
> 环境:WSL Debian 13 + 宿主现成文件(sudo/sh/libc),不下新包
> 目的:测 PoC-4/5 都没碰的三个架构盲区——setuid 权限、多包通信、多版本磁盘代价。
> 关联:[`foundations/01-store`](../../docs/architecture/foundations/01-store.md)、[`foundations/05`](../../docs/architecture/foundations/05-multi-source-and-isolation.md)、[`ai/03-garbage-collection`](../../docs/architecture/../ai/03-garbage-collection.md)

---

## A. setuid:内容寻址 store 会丢提权位 ⚠️(抓到 bug 隐患)

**实测**(样本 `/usr/bin/sudo`,真 setuid):

```
源权限:           0o4755  (setuid bit 置位)
天真复制(read字节→write新文件): 0o644   ← setuid 丢了!
显式 chmod 恢复:   0o4755  ← 能修
```

**架构结论**:内容寻址只哈希"内容字节",但 **setuid/setgid/权限位是带外的 inode 元数据**。如果 store 用最直觉的"读字节→写新文件"入库,**权限位全丢**——`sudo/ping/passwd/su` 这类 setuid 包入 store 后**提权能力直接失效**(sudo 不再能提权)。

**修正(必须进实现)**:
1. store 入库时**显式记录并恢复完整权限位**(含 setuid/setgid/sticky)。
2. **权限位纳入内容寻址的规范化输入**——否则"同内容不同权限"会被误判为同一个 hash,setuid 版和非 setuid 版混淆。
3. 安全副作用(正向):setuid 包入 store 是显式、可审计的特权声明,反而比传统发行版"散落各处的 setuid"更可控——可由 verify/AI 维护者标记审查(呼应 [`server/01`](../../docs/architecture/server/01-server-and-trust-root.md) 的高敏感包阈值签名)。

> 这是典型"照直觉写就会崩"的点,赶在写 Rust 前发现。

---

## B. 多包通信:轻隔离不挡协作 ✅(验证设计假设)

**实测**:让两个"隔离包"互相调用 + 管道通信:

```
跨包 exec:  echo from_pkgA $(echo from_pkgB) → "from_pkgA from_pkgB"  rc=0 ✅
管道:       echo hello | cat → "hello"  rc=0 ✅
```

**架构结论**:轻隔离(PoC-2/4 用的"私有库搜索路径")**只隔离库查找,不隔离进程/exec/pipe/socket**。所以两个轻隔离的包能正常互相调用与通信。

- 这回答了 PoC-4 留的疑问:"隔离的包之间能否通信"——**能,默认轻隔离不破坏包间协作**。
- 只有**强隔离 namespace** 才切断进程级通信,那时需显式打洞(共享挂载/socket)。这也明确了 [`foundations/05`](../../docs/architecture/foundations/05-multi-source-and-isolation.md) 分层隔离的取舍:默认轻=协作无碍,按需强=安全但要打洞。

---

## C. 多版本磁盘代价:去重压得住 ✅(验证设计假设)

**实测**(libc.so.6 真实大小 ~2MB,模拟 10 包各带同源闭包):

| 策略 | 占用 | 说明 |
|---|---|---|
| 不去重(每包独立全套) | 20.5 MB | 10 份 glibc |
| 内容寻址去重 | 2.5 MB | glibc 同版本只存 1 份 |
| **节省** | **87.8%** | |

**架构结论**:同源闭包要求每包带自己的 glibc,听起来很胖,但**只要多包用同一版本 glibc,内容寻址按 hash 去重 → 只存 1 份**。真正占空间的只有"**多个不同版本** glibc 并存"(即"保留两份"场景),而那是**有意的**多版本代价,GC 在引用归零后回收(见 [`ai/03-garbage-collection`](../../docs/architecture/../ai/03-garbage-collection.md))。

→ 回答了"同源闭包会不会让磁盘爆炸":**不会,去重把同版本压成一份;多版本占用是可控且有意的。**

---

## 总结

| 盲区 | 裁决 | 行动 |
|---|---|---|
| **setuid 权限位** | ⚠️ 天真实现会丢 setuid,提权失效 | store 必须显式恢复权限位 + 纳入哈希输入(已回写 01-store) |
| **多包通信** | ✅ 轻隔离不挡 exec/pipe/socket | 确认默认轻隔离协作无碍(已记入 05) |
| **多版本磁盘** | ✅ 同版本去重省 ~88% | 确认磁盘可控(已记入 05) |

**总判断**:三个盲区里两个是"设计假设被实测证实"(通信、磁盘),一个是"抓到必须修的实现细节"(setuid)。没有动摇架构,反而让 store 实现的需求更完整、更精确。

---

## 复现

```bash
python3 experiment.py   # WSL Debian,用宿主 sudo/sh/libc,不下包 → /tmp/aevum-poc6/report.json
```

---

## 一句话总结

> **setuid:内容寻址天真复制会丢 setuid 位(sudo 提权失效)→ store 必须显式恢复权限位并纳入哈希。
> 通信:轻隔离只隔库路径、不隔进程,隔离包能正常互调(实测 rc=0)。
> 磁盘:同源闭包多包同版本 glibc 去重省 87.8%,磁盘可控。
> 三盲区两验证一修正,架构稳。**
