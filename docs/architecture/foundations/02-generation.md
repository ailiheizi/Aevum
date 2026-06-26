# Generation —— 不可变的系统世代

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:[`01-store.md`](01-store.md)、状态机 [`../runtime/01-generation-lifecycle.md`](../runtime/01-generation-lifecycle.md)、回滚与修复 [`../../ai/`](../../ai/)

---

## 0. 设计哲学

> **每一次系统变更都凝固成一个不可变的世代;旧世代永不修改,永远可回退。**

`Aevum`(世代)这个名字就来自这里。世代是回滚、可复现、AI 试错(错了秒回)的载体。

---

## 1. 是什么

一个 Generation 是某一刻系统的完整、不可变快照:它精确地说明"哪些包(哪些 hash)以什么布局构成了这个系统"。

```text
<aevum_root>/generations/
├── gen-001/
│   ├── lock.toml              该世代的完整锁定快照（闭包 + 精确 hash）
│   ├── manifest.toml          元数据（创建时间、来源、父世代、模板、Layer 信息）
│   ├── layers/                按层组织的链接（见 §3）
│   │   ├── foundation/        → 链接到 store 内 foundation 包的 hash
│   │   ├── system/            → 系统层包
│   │   └── app/               → 软件层包
│   └── meta/
│       ├── parent             父世代编号（回滚链）
│       ├── source             创建来源：cli / gui / ai / import / upgrade
│       ├── verified           是否通过 verify
│       └── trust              信任级别
├── gen-002/                   下一个世代
└── active → gen-002           ← 原子指针：当前激活的世代
```

---

## 2. 世代的内容

每个 `gen-N/`:

- **lock.toml** —— 完整锁定快照。这是可复现的核心:拿着它能在任何机器重建逐字节一致的世代。结构见 [`../runtime/02-intent-resolved-lock.md`](../runtime/02-intent-resolved-lock.md)。
- **manifest.toml** —— 元数据(下方 §5)。
- **layers/** —— 按 Foundation / System / App 三层组织的链接树,指向 store 内的 hash 目录(见 [`../../layers/`](../../layers/))。
- **meta/** —— 回滚链、来源、验证状态、信任级别。

---

## 3. 为什么按 Layer 组织链接

世代内部不是一个扁平的包列表,而是分三层链接。这让"稳定层不被软件层炸穿"在世代结构上就有体现:

- 校验世代时,可以单独断言 "所有 required 的 foundation 包都在且版本精确匹配"。
- 回滚/修复时,可以只动 app 层而保 foundation/system 不变。
- 极端兜底时,可构造一个只含 foundation 层的最小世代。

详见 [`../../layers/01-foundation.md`](../../layers/01-foundation.md)。

---

## 4. active 指针与原子性

```text
generations/active → gen-N
```

- **激活** = 改写 `active` 指针指向新世代。这是单个原子操作(symlink 原子替换 / 原子写文件)。

> **实证**:[`PoC-7`](../../../poc/poc7-core-mechanics/REPORT.md) 用真文件+真 symlink 验证:`os.rename` 替换 active symlink 是原子的,切换耗时 0.09ms,系统在任一时刻只看到旧或新世代、无半切状态;回滚指回旧世代 0.095ms 不重建。原子切换与瞬时回滚卖点成立。
- 切换前后系统要么是旧世代、要么是新世代,**不存在中间半装状态**(这正是 NixOS "atomic" 的精髓)。
- 切换后通知运行中的实例热重载。
- 旧的 active 世代不删,留作回滚目标。

---

## 5. manifest 数据结构(草案)

```toml
# generations/gen-N/manifest.toml
schema_version = "1.0"

[generation]
id = 7
created_at = "2026-06-08T12:00:00Z"
parent = 6                       # 回滚链：上一个世代
source = "ai"                    # cli / gui / ai / import / upgrade
verified = true
trust = "normal"                 # foundation-only / normal / dev-unsealed

[intent]
# 触发这次世代的意图摘要（人类可读）
summary = "加入 PostgreSQL 16"
template = "server-db"

[stats]
package_count = 142
new_hashes = 5                   # 相对父世代新增的 hash 数
closure_id = "clo-9f86d081"      # 闭包指纹（同 closure → 同 id）
```

---

## 6. 世代与回滚链

```text
gen-001 ←─ gen-002 ←─ gen-003 ←─ gen-004(active)
  │           │          │           │
 parent 链让任意世代都能找到它的前驱
```

- 回滚 = 把 `active` 指回某个历史 verified 世代,**不重新求解**,直接用历史 lock,秒级完成。
- 默认回滚到 `parent`,也可跳回任意指定的 verified 世代。
- 回滚本身**不销毁**被回滚掉的世代(它仍可被"前滚"回去),直到 GC 判定无用。

---

## 7. 世代与 GC 的关系

世代是 GC 的"根":GC 通过扫描"活跃世代集合"引用了哪些 store hash,来判定哪些 hash 可回收。保留策略(保留最近 N 个世代、保留失败修复 M 天等)针对的就是世代。详见 [`../../ai/03-garbage-collection.md`](../../ai/03-garbage-collection.md)。

---

## 8. 不变量(实现必须保证)

1. 世代一旦 verified,其 lock 与链接树**永不修改**。
2. `active` 永远指向一个 verified 世代(或 foundation-only 兜底世代)。
3. 激活是原子的,无中间态。
4. 被某个保留中的世代引用的 store hash,GC 绝不回收。
5. 每个世代可独立校验"foundation 层完整且版本精确"。

---

## 9. 验收清单

- [ ] 世代目录结构(lock/manifest/layers/meta)完整
- [ ] active 原子切换(并发安全)
- [ ] parent 回滚链可追溯
- [ ] 回滚不重新求解,直接用历史 lock,< 1 秒
- [ ] 回滚后可前滚
- [ ] 世代级 foundation 完整性断言
- [ ] foundation-only 兜底世代可构造并激活
