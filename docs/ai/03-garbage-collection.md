# 垃圾回收 —— 引用统计与回收

> 父文档:[`README.md`](README.md)
> 关联:store [`../architecture/foundations/01-store.md`](../architecture/foundations/01-store.md)、世代 [`../architecture/foundations/02-generation.md`](../architecture/foundations/02-generation.md)
> 机制原型:RELIK 的 GC 模块 + NixOS `nix-collect-garbage`

---

## 0. 设计哲学

> **世代是根,引用是边。没有任何活跃世代引用的 hash,才是垃圾。
> 多版本并存的代价由 GC 来还 —— 但绝不误删还在用的东西。**

"保留两份"让 store 长胖,GC 是它的对偶:按引用统计安全地把真正没人用的回收掉。

---

## 1. 核心模型:可达性

```text
GC roots（根）= 活跃世代集合
              = active 世代 + 最近保留的 N 个世代 + 保留期内的失败修复世代
        ↓ 引用
每个世代的 lock.toml 列出它引用的所有 store hash
        ↓ 求并集
referenced = ∪（所有活跃世代引用的 hash）
        ↓
garbage = store 内所有 hash − referenced
```

只要一个 hash 还被任何 GC root 可达地引用,就**绝不回收**。这保证回滚目标永远完整。

> **实证**:[`PoC-7`](../../poc/poc7-core-mechanics/REPORT.md) 在真 store 上验证:两个世代共享 libc,删掉 gen-2 后,仅 gen-2 引用的 python3.12 被回收,而 gen-1 仍引用的 **libc 正确保留、未被误删**,active 的 python3.11 也保留。可达性 GC 不误删共享依赖——这是 GC 最易写错、后果最重的点,实测算法正确。

---

## 2. GC 算法

```text
fn collect_garbage(opts) -> GcReport:
    1. all = 列出 store 内所有 hash
    2. roots = 计算 GC roots：
         active 世代
         + 最近 keep_recent 个 verified 世代
         + 保留期内的失败修复世代
         + 被钉住(pinned)的世代
    3. referenced = roots.flat_map(|g| g.lock.referenced_hashes()).collect_set()
    4. garbage = all.filter(|h| !referenced.contains(h))
    5. 若非 dry_run 且(用户确认 或 auto_gc)：删除 garbage
    6. 返回 GcReport { removed, freed_bytes, kept_generations }
```

> Foundation 包几乎总被 active 引用,所以正常不会被回收;即便某历史世代不再引用某 foundation 旧版本,只要没有活跃世代用它,才会被收 —— 这是安全的,因为当前 foundation 由 manifest 锁定且在场。

---

## 3. 保留策略

```toml
# 用户可配置的 GC 策略
[gc]
keep_recent_generations = 10        # 至少保留最近 10 个世代(回滚余量)
keep_failed_repairs_days = 7        # AI 修复失败的世代保留 7 天(便于排查)
auto_gc = true                      # 自动 GC 开关
auto_gc_when_disk_above = "5GB"     # 磁盘超阈值自动触发
protect_pinned = true               # 被钉住的世代永不回收
```

- **keep_recent**:回滚的安全余量。设大些更安全,代价是占盘。
- **pinned**:用户可"钉住"某个已知好用的世代,GC 永不碰(例如一个验证过的生产基线)。
- **失败修复保留期**:失败的候选世代留几天供 AI/用户分析,过期回收。

---

## 4. 触发时机

| 触发 | 说明 |
|---|---|
| 手动"一键清理" | 用户主动 |
| 定期巡检 | 后台(如每周)扫描提议 |
| 磁盘超阈值 | `auto_gc_when_disk_above` 触发 |
| Maintainer 主动 | 发现冗余多版本残留时提议(关键删除仍问人) |

GC **默认 dry-run 先报告**,删除需用户确认或显式开启 auto_gc。

---

## 5. GC 与"保留两份"的平衡

```text
保留两份让 store 出现：openssl@3.2(被 app-X 用) + openssl@3.0(被 app-Y 用)
   ↓
若某天 app-Y 被卸载，新世代不再引用 openssl@3.0
   ↓
但旧世代（保留期内）可能还引用它 → 暂不回收
   ↓
等旧世代滑出 keep_recent 窗口、无任何 root 引用 openssl@3.0
   ↓
GC 安全回收它，归还那"第二份"的磁盘
```

→ "保留两份"是**临时代价**,GC 在引用真正归零后自动归还。用户不会永久为历史冲突买单。

---

## 6. GC 日志

```text
gc/gc.log
2026-06-08T10:00:00 manual        removed=12 freed=850MB kept_gens=10
2026-06-09T03:00:00 auto_disk     removed=3  freed=120MB kept_gens=10
2026-06-15T03:00:00 weekly_scan   removed=0  freed=0      kept_gens=10  note="无可回收"
```

每次 GC 记录:触发源、删了多少、释放多少、保留世代数。可审计、可回溯。

---

## 7. 一键清理 UI(示意)

```text
┌──────────────────────────────────────┐
│  🧹 存储管理                          │
├──────────────────────────────────────┤
│  已用空间：2.3 GB                     │
│  ├─ 当前激活：1.5 GB                  │
│  ├─ 历史世代（可回滚）：500 MB         │
│  ├─ 保留两份的冗余版本：300 MB         │
│  └─ 可清理：300 MB                    │
│                                      │
│  可清理明细：                         │
│  · 旧世代残留(已滑出保留窗口) → 200MB │
│  · 过期失败修复(3 个) → 80MB          │
│  · 过期下载缓存 → 20MB                │
│                                      │
│  设置：                               │
│  ☑ 保留最近 10 个世代                 │
│  ☑ 磁盘 > 5GB 自动清理                │
│  📌 已钉住 2 个世代（不清理）          │
│                                      │
│  [一键清理]  [仅查看(dry-run)]        │
└──────────────────────────────────────┘
```

---

## 8. 安全不变量

1. 被任何 GC root 可达引用的 hash **绝不回收**。
2. active 世代引用的一切永不回收。
3. pinned 世代及其引用永不回收。
4. 删除默认需确认(或显式 auto_gc),不静默删。
5. GC 不破坏任何 verified 世代的完整性(回滚永远可用)。

---

## 9. 验收清单

- [ ] 可达性算法正确(不删活跃引用)
- [ ] GC roots 计算含 active + keep_recent + 保留期失败修复 + pinned
- [ ] 保留策略可配置并生效
- [ ] dry-run 默认 + 删除需确认
- [ ] "保留两份"在引用归零后能被回收
- [ ] pinned 世代永不回收
- [ ] GC 日志完整可审计
- [ ] GC 后所有保留世代仍可回滚
