# 宿主共存 —— Aevum 如何与 Linux 宿主和平相处

> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:定位 ADR [`../adr/0001-positioning-vs-nixos.md`](../adr/0001-positioning-vs-nixos.md)、世代 [`../foundations/02-generation.md`](../foundations/02-generation.md)、二进制兼容 [`../runtime/03-binary-compat.md`](03-binary-compat.md)
> 回应:设计评审 H7(宿主共存架构缺失,ADR-0001 用"待实现阶段定"跳过)

---

## 0. 设计哲学

> **Aevum 是建在 Linux 之上的用户态系统层(ADR-0001)。
> 它必须能装进一个已有的 Linux 里、与宿主和平共处、随时干净卸载 ——
> 不接管 `/usr`,不污染宿主,不做"只能进不能出"的霸王安装。**

评审 H7 点明:"比 NixOS 轻松"主要兑现在第一小时,但世代如何投影到 PATH、如何不破坏宿主 `/usr`、如何干净卸载,全缺。本篇补上。

---

## 1. 问题:用户态系统层落在哪、怎么不打架

ADR-0001 定 Aevum 为用户态系统层,复用宿主内核与文件系统。那么具体:

- Aevum 装的软件,怎么让用户在 shell 里调用到?
- 会不会和宿主的 `apt`/`pacman` 装的东西冲突?
- 不想用了,能干净删掉吗?

---

## 2. 落点:一切在 Aevum 根目录内,不碰宿主系统目录

```text
宿主 Linux:
  /usr /lib /bin …          ← Aevum 只读引用,绝不写入/覆盖
  /home/<user>/             ← 用户家目录

Aevum 根（默认在用户可控位置,如 ~/.aevum 或 /opt/aevum）:
  store/                    ← 内容寻址存储(见 foundations/01)
  generations/              ← 世代(见 foundations/02)
  active → gen-N
  profile/                  ← 当前 active 世代投影出的"可用视图"
    bin/  lib/  share/ …    ← 指向 store hash 的链接
```

**铁律**:Aevum 的写操作全部限制在自己的根目录内。宿主的 `/usr`、`/bin`、`/lib` 对 Aevum 是**只读引用**(用于二进制兼容的基线回退),绝不写入或覆盖。

---

## 3. 投影到 PATH:世代如何"可用"

```text
active 世代 → 生成 profile/bin 等视图(链接到 store 内 hash)
        ↓
用户 shell 的 PATH 前置 Aevum profile:
  export PATH="$AEVUM_ROOT/profile/bin:$PATH"
        ↓
效果:
  - Aevum 装的工具优先被找到
  - 没被 Aevum 接管的命令,落到宿主 PATH(共存)
  - 切世代/回滚 → profile 视图原子更新 → PATH 内容随之变(无需改 PATH 本身)
```

- **opt-in 激活**:PATH 注入通过用户 shell 配置(一行 source),用户明确选择启用,而非偷偷改全局。
- **per-shell / per-project 作用域**:可只在某个 shell、某个项目目录激活某世代(类似 direnv),不影响全局。
- 切换世代不动 PATH 字符串本身,只换 profile 指向 → 和世代原子切换一致。

---

## 4. 与宿主包管理器共存

| 场景 | 处理 |
|---|---|
| 宿主 `apt` 装了 git,Aevum 也装了 git | PATH 优先级决定用谁;两者物理隔离,不互相覆盖 |
| Aevum 软件需要某基线库 | 优先用 Aevum store 内的;必要时只读引用宿主 `/lib`(见 [二进制兼容](03-binary-compat.md)) |
| 宿主升级了系统库 | 不影响 Aevum 世代(世代锁定自己的 hash,可复现不受宿主漂移影响) |

→ Aevum 和宿主包管理器是**两套并行的世界**,通过 PATH 优先级与只读引用协作,而非争夺同一批系统目录。这也避免了 NixOS"接管整个系统"的重量级心智。

---

## 5. 干净卸载

```text
卸载 Aevum：
1. 从 shell 配置移除 PATH 注入那一行
2. 删除 $AEVUM_ROOT 整个目录(store/generations/profile 全在里面)
3. 完成 —— 宿主系统目录从未被写过,无残留
可选:
  - 状态子卷(见 runtime/04)若在 Aevum 根外,提示用户单独处理
  - 提供 `aevum uninstall` 一键完成上述并列出任何根目录外的关联物
```

**因为铁律(§2)保证 Aevum 只写自己的根**,卸载就是"删一个目录 + 撤一行 PATH",不会像某些工具那样在系统各处留下残骸。这是"能进能出"的承诺。

---

## 6. 多用户与权限

```text
- 默认 per-user 安装(~/.aevum):无需 root,降低门槛与风险
- 可选系统级安装(/opt/aevum):多用户共享 store,需管理员;
  仍不写宿主 /usr,只在 /opt/aevum 内
- store 内容只读 + 内容寻址 → 多用户共享同一 hash 安全(去重红利)
```

per-user 默认免 root,直接回应"比 NixOS 轻松"——NixOS 通常要接管系统、要 root;Aevum 可以一个普通用户在家目录里跑起来。

---

## 7. 与 ADR-0001 的关系

ADR-0001 定了"用户态系统层"的定位但把共存细节留给实现期。本篇把被跳过的部分补成明确设计:落点(§2)、PATH 投影(§3)、共存(§4)、卸载(§5)、权限(§6)。未来若演进为可引导发行版(ADR-0001 §后续条件),宿主共存模式仍可作为"在现有 Linux 上试用 Aevum"的入口形态保留。

---

## 8. 边界与诚实声明

- 本篇定共存的**架构与契约**;PATH 注入的具体 shell 兼容(bash/zsh/fish)、profile 视图的实现(symlink farm vs overlay)待实现期。
- "只读引用宿主 `/lib`"用于二进制兼容兜底,其稳定性受宿主影响 —— 这是用户态层的固有取舍(见 [二进制兼容](03-binary-compat.md) §5 诚实声明)。
- 系统级关键服务(如取代宿主 init)不在共存模式范围;共存模式定位是"在宿主之上加一层",不是"取代宿主系统层"。

---

## 9. 验收清单

- [ ] Aevum 写操作严格限制在 $AEVUM_ROOT 内
- [ ] 宿主 /usr /bin /lib 只读引用,绝不写入
- [ ] active 世代投影 profile 视图,PATH 前置注入(opt-in)
- [ ] per-shell / per-project 作用域激活
- [ ] 切世代/回滚 → profile 原子更新
- [ ] 与宿主包管理器共存(PATH 优先级,物理隔离)
- [ ] 干净卸载:删根目录 + 撤 PATH,宿主无残留
- [ ] per-user 默认免 root;可选系统级
