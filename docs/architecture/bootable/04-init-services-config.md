# 阶段4:真 init + 服务管理 + 系统配置(设计草案)

> 父文档:[`../adr/0006-bootable-distribution.md`](../adr/0006-bootable-distribution.md)(阶段4+ 在其"分阶段路线"第76行立项)
> 关联:Foundation 层 [`../../layers/01-foundation.md`](../../layers/01-foundation.md)(`init` 已是 Foundation 钦定组件)、状态回退语义 [`../runtime/04-state-vs-package-rollback.md`](../runtime/04-state-vs-package-rollback.md)、世代模型 [`../foundations/02-generation.md`](../foundations/02-generation.md)
> 红线:ADR-0002(不强制图灵完备 DSL,配置默认纯数据 TOML)、ADR-0003(AI 产约束、求解器算 hash)
> 参考:s6 / s6-rc(skarnet)、dinit、NixOS(systemd unit 由表达式生成 + environment.etc)、ostree(/etc 三路合并)
>
> **状态:设计草案,待评审。尚未进入实现。** init 路线已选定 s6/s6-rc(见 §2)。

---

## 0. 这一阶段要解决什么

阶段1-3 让 Aevum 世代能在 QEMU 里被引导为系统根、有了多世代菜单 + 命令式回滚。但 PID1 仍是 **busybox 占位**:它只 `--install` 一堆 applet、挂 proc/sys、exec 一个 shell——**没有服务管理、没有系统配置**。

阶段4 把"能开机进 shell"演进成"像个真发行版":

1. **真 init(PID1)**:可靠地 reap 孤儿进程、按依赖顺序拉起/停止服务、转交兜底 shell。
2. **服务管理**:声明式描述服务(TOML),编译成 init 能执行的服务定义,支持依赖、启停、状态查询。
3. **系统配置**:`/etc`、网络、fstab、locale、用户——不可变世代基底 + 可变覆盖。

**不在本阶段**:完整 systemd 兼容、桌面会话、复杂网络栈(NetworkManager 级)、容器运行时。这些是更后面的长尾(ADR-0006 已声明阶段4+ 不一次做完)。

---

## 1. 必须对齐的既有约束(不是新发明,是落地已有的槽位)

| 约束来源 | 内容 | 对阶段4 的含义 |
|---|---|---|
| Foundation 层(layers/01) | `init` 已列为 Foundation `required=true` 组件,签名保护,`upgrade_policy=on-major` | init 不是新立项,是填 Foundation 早留的槽。init 二进制须能被 Foundation manifest 管理 |
| min-toolset "最后防线"(layers/01 §40) | 兜底工具**静态链接**,不依赖 System 兼容层 | PID1 + 兜底 shell 必须静态可链接;基线坏了兜底也能起 |
| 状态回退契约(runtime/04) | 世代 manifest 记 `[[state]] service=.. snapshot=..`,FS 子卷快照与世代绑定,回滚作事务 | 服务的持久状态随世代回退,本阶段服务模型须能挂接这套契约 |
| ADR-0002 红线 | 配置默认纯数据 TOML,不强制图灵完备 DSL | 服务/配置用 TOML 声明,**不**引入 unit 文件 DSL 或脚本语言当配置入口 |
| ADR-0006 边界 | 引擎零改动、只加外壳;每阶段独立可验证;失败不影响用户态包管理价值 | 服务/配置编译器是新 crate/外壳,不动 store/generation/solver 核心 |

---

## 2. init 路线:为什么选 s6 / s6-rc

### 2.1 候选对比

| 方案 | PID1 可独立 | 静态链接 | 依赖式服务 | 声明式友好 | 与世代化哲学契合 | 采用者 | 排除/选用 |
|---|---|---|---|---|---|---|---|
| busybox init(现状) | 是 | 是 | ✗(只跑 inittab) | 弱 | 占位级 | 嵌入式 | 现状,要替换 |
| **s6 + s6-rc** | 是(s6-svscan) | **是** | **是(s6-rc 编译期算依赖图)** | **强(服务=目录,易由 TOML 生成)** | **高** | Artix、Obarun | **首选** |
| dinit | 是 | 是 | 是(内建) | 中 | 高 | Chimera Linux | 次选 |
| runit | 是 | 是 | 弱(无显式依赖图) | 中 | 中 | Void | 备选 |
| OpenRC | 否(需配 PID1) | 部分 | 是 | 中(shell 脚本) | 中(脚本味重) | Gentoo、Artix | 不选 |
| systemd | 是 | ✗(动态依赖庞大) | 是 | 强但自成 DSL | **低(重、抢配置权、耦合)** | 主流 | **排除** |
| 自写 PID1 | 是 | 是 | 要自造 | 自定义 | 最高但工程量最大 | — | 不选(重造轮子) |

### 2.2 选 s6/s6-rc 的理由(结合 Aevum)

- **机制分层、PID1 极简**:s6-svscan 当 PID1 只做监督树根;服务状态机、依赖、日志各是独立小工具。崩面小,呼应"PID1 崩=系统崩"的护栏需求。
- **可静态链接**:满足 Foundation min-toolset"最后防线"。
- **s6-rc 的编译期依赖图最契合 Aevum 范式**:s6-rc 把"服务源定义"**编译**成一个不可变的 service database。这与 Aevum"声明式意图 → 确定性求解 → 不可变产物"是**同构**的——我们的 TOML 服务声明 → 编译成 s6-rc source → `s6-rc-compile` 出 db,整个 db 可内容寻址、随世代走、随世代回退。
- **服务定义是静态文件/目录**,不是图灵完备脚本,天然符合 ADR-0002。

### 2.3 排除 systemd 的理由(客观)

systemd 技术上成熟、生态最大,但:动态链接依赖庞大(违 min-toolset 静态原则);把服务/挂载/日志/登录/网络耦合进一套(违"机制分层");它自己想当配置与状态中心(与 Aevum 世代化配置**抢权**);unit 是自成体系的声明 DSL,且 drop-in/generator 引入隐式行为(违"配置可被确定性求解")。与 Aevum 哲学正面冲突,排除。**诚实补充**:代价是放弃 systemd 庞大的现成 unit 生态,很多上游软件默认只发 systemd unit;Aevum 需自己维护服务声明或做 unit→TOML 的转译(后续长尾)。

---

## 3. 服务声明模型(TOML → s6-rc)

### 3.1 设计原则

- 用户/AI 写**纯数据 TOML**(ADR-0002),描述"要什么服务、依赖什么、怎么起"。
- Aevum 的**服务编译器**(新外壳)把 TOML → s6-rc source 目录 → `s6-rc-compile` → 不可变 service db → 入 store、随世代走。
- TOML **不是**图灵完备脚本;`run` 是 argv 数组(可加少量受控模板变量),不是嵌入 shell 逻辑的入口。

### 3.2 示意 schema

```toml
# services/postgresql.toml （世代内声明,纯数据)
[service]
name = "postgresql"
type = "longrun"             # longrun(常驻) / oneshot(一次性) / bundle(服务组)
description = "PostgreSQL 数据库"

[run]
# argv 数组,不是 shell 字符串(避免注入 + 保持纯数据)。
argv = ["/usr/bin/postgres", "-D", "/var/lib/postgresql/data"]
user = "postgres"            # 降权运行
env = { PGDATA = "/var/lib/postgresql/data" }

[deps]
# 依赖的其它服务(s6-rc 编译期算拓扑序)。
after = ["network", "mount-data"]
needs = ["mount-data"]       # 硬依赖:它没起来本服务不起

[state]
# 挂接 runtime/04 的状态回退契约:本服务的持久数据目录 → 随世代快照回退。
persist = ["/var/lib/postgresql/data"]
snapshot_with_generation = true

[health]
# 可选:就绪探测(s6 的 readiness fd / notification)。
readiness = "fd"             # fd / tcp:5432 / none
```

### 3.3 映射到 s6-rc

| TOML 字段 | s6-rc source 产物 |
|---|---|
| `type=longrun` | `type` 文件填 `longrun` + `run` 脚本 |
| `run.argv` | `run` 脚本(execline 或 `#!/bin/execlineb`,argv 直填,不引 shell) |
| `run.user` | `run` 里用 s6 的 `s6-setuidgid` 降权 |
| `deps.after/needs` | `dependencies.d/` 下放依赖名 |
| `type=bundle` | `contents.d/` 列成员 |
| `state.persist` | 写进世代 manifest 的 `[[state]]`(runtime/04),不进 s6 db |

> 关键:`run` 脚本用 **execline**(s6 配套,非图灵完备、无变量展开陷阱)而非 bash,保持"配置即数据"。argv 由 TOML 直接生成,不让用户写 shell。

---

## 4. 系统配置:/etc 不可变基底 + 可变覆盖

### 4.1 问题

`/etc` 天生半可变:大部分(passwd 骨架、服务配置、locale)应由世代声明式生成、可回退;少数(机器 id、手改的网络配置、用户加的 host)是本地可变状态。直接把 `/etc` 塞进不可变世代根会让本地修改无处落、且每次配置变更要重建世代。

### 4.2 方案:声明式生成基底 + overlay 可变层(对标 ostree / NixOS)

```text
/etc(运行时可见)
  = overlayfs:
      lower(只读)= 世代生成的 /etc 基底（内容寻址,随世代回退)
      upper(可写)= 本地可变层（持久卷,按 runtime/04 分类处理回退)
```

- **基底**:Aevum 的配置编译器把 TOML 声明(用户/网络/fstab/locale/服务配置)→ 生成 `/etc` 文件 → 内容寻址入 store → 世代引用。对标 NixOS `environment.etc` 把声明编译成 `/etc` 链接树。
- **可变层**:overlayfs upper,持久。机器 id、本地 host、运行时改动落这里。
- **回退**:基底随世代 active 指针回退(已有机制);可变层按 runtime/04 的三类(可重建/关键有状态/纯本地)分别处理——纯本地默认不回退,避免"回退把用户手改的东西抹了"。
- **不支持 overlayfs 的环境**:降级为"世代生成的 /etc 直接铺 + 本地改动告警",诚实标注。

### 4.3 activation(声明 → 运行态)

NixOS 有 activation script 把声明落成运行态。Aevum 对应一个**受控的 activate 步骤**(不是任意脚本):
- 重建 `/etc` overlay 的 lower。
- 把变更的服务声明重编译 s6-rc db、`s6-rc -u change` 应用差异(只动变化的服务,不重启全部)。
- 这一步是引擎驱动的确定性过程,不是用户写的 shell。

---

## 5. 架构落点(引擎零改动,只加外壳)

```text
新增外壳(不动 store/generation/solver 核心):
  crates/service-compiler/   TOML 服务声明 → s6-rc source → 调 s6-rc-compile → db
  crates/etc-builder/        TOML 系统配置 → /etc 基底文件树(内容寻址入 store)
  CLI:
    aevum service compile <gen>     编译世代内服务声明为 s6-rc db
    aevum etc build <gen>           生成世代的 /etc 基底
    aevum activate <gen>            (扩展)切世代 + 重建 /etc overlay + s6-rc 应用差异

引擎侧(已有,复用):
  generation manifest 扩展 [[state]]（runtime/04 已定）
  Foundation manifest 纳入 init(s6) + min-toolset（layers/01 已定)
```

调系统工具不引依赖(沿用项目铁律):`s6-rc-compile`、`s6-svscan`、`mount`(overlay)都是外部二进制,Aevum 调它们,不把逻辑塞进 Rust 依赖。

---

## 6. 建议的子阶段拆分(每步独立 QEMU 可验证)

> ADR-0006 要求每阶段独立交付。阶段4 本身再拆:

- **4a:s6 接管 PID1**。把 busybox-as-PID1 换成 s6-svscan,QEMU 验证它能起监督树 + 拉起一个 demo longrun 服务(如一个每秒打印的 daemon)+ 转兜底 shell。**最小、不碰 TOML 编译器**(服务定义先手写 s6 source)。
  - **✅ 已实证(CHANGELOG 第27轮)**:`scripts/build-s6-boot.sh`,gen-60 装 Debian s6+execline 闭包,s6-svscan 作 PID1、demo 服务被 `s6-svstat` 确认 `up`。
  - **暴露真引擎缺口**:补闭包未按 ELF SONAME 建库软链(`libX.so.A.B.C` 实体 vs NEEDED 的 SONAME `libX.so.A`),loader 找不到库 → s6 首次引导 panic。4a 脚本侧临时补链;引擎修复是 4b 前优先待办。
  - **Debian trixie 无 s6-rc 包**:只有 s6 监督工具,无依赖式服务管理器 s6-rc。4a 不受阻;4b 须先解决 s6-rc 来源(上游静态构建,或改用 s6 原生 scandir 机制)。
- **4b:服务编译器**。TOML 服务声明 → s6-rc source → db。QEMU 验证从 Aevum 世代里的 TOML 起服务。
  - **✅ 已实证(CHANGELOG 第29轮)**:新增 `crates/service-compiler`(`Service::parse` + `compile_service`,内置零依赖极简 TOML 子集解析器),CLI `aevum service compile`,`examples/services/demo.toml`。QEMU:`[demo-svc] 由 TOML 声明编译` 服务 `s6-svstat` 确认 up。
  - **起步选 s6 原生 scandir(非 s6-rc)**:Debian 无 s6-rc 包。当前 deps 是元数据,无编译期依赖图。s6-rc 依赖图编排留作增强(前置:解决 s6-rc 来源)。
- **4c:/etc 基底生成 + overlay**。TOML 系统配置 → /etc 基底,overlay 可变层。
  - **✅ 已实证(CHANGELOG 第30轮)**:新增 `crates/etc-builder`(`build_etc` → /etc 基底文件树),CLI `aevum etc build`,`examples/etc/system.toml`。QEMU:`/etc = overlay`(lower=世代生成只读基底 + upper 可变层),hostname 来自声明、本地写入落 upper 不污染基底。
  - **修了真问题**:该内核 overlayfs 是模块非内建 → 从 linux-image 包取 `overlay.ko` 进世代根 + init `insmod`(原降级到 cp 铺基底,查根因后改对)。
  - upper 用 tmpfs 演示;真部署用持久卷 + runtime/04 状态回退分类(4d)。
- **4d:服务状态随世代回退**。挂接 runtime/04 的快照契约,验证回滚一个有状态服务时状态一致。
- **4e+:长尾**。网络、用户管理、locale、fstab 细化,按需推进。

---

## 7. 边界与诚实声明

- **QEMU,非物理机**(沿用阶段1-3 边界)。
- **放弃 systemd 生态的代价**:上游多数软件只发 systemd unit,Aevum 需自维护服务声明或做转译,这是真实工程负担,长尾持续。
- **静态链接 s6 需验证**:s6 官方支持静态,但与具体 libc(glibc/musl)和 Aevum 的同源约束(里程碑4)交互需实测;可能要 musl 静态构建。
- **overlayfs + 世代回退的事务性**未在本文解决到实现级(runtime/04 已声明这是实现期硬课题)。
- **服务就绪/健康探测**只列了接口,完整实现是长尾。
- **不可逆操作护栏**(runtime/04 §5)对有状态服务同样适用,激活前警告。

---

## 8. 验收清单(本设计文档评审用)

- [ ] init 路线选定理由充分、systemd 排除有据(§2)
- [ ] 服务 TOML schema 是纯数据、不引图灵完备 DSL(§3,对齐 ADR-0002)
- [ ] 服务模型挂接了 runtime/04 的状态回退契约(§3.2 `[state]`)
- [ ] /etc 方案处理了"不可变基底 + 可变覆盖 + 回退分类"(§4)
- [ ] 架构落点确认引擎零改动、只加外壳(§5,对齐 ADR-0006)
- [ ] 子阶段拆分每步独立 QEMU 可验证(§6)
- [ ] 边界诚实(systemd 生态代价、静态链接待验、事务性待实现)(§7)

---

## 下一步(评审通过后)

从 **4a(s6 接管 PID1)** 起步——最小、不碰编译器、QEMU 可立即验证,延续阶段1-3 的增量风格。
