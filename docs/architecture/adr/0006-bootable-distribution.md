# ADR-0006:可引导发行版增量项目(bootable-aevum)

> 状态:**已接受(Accepted)** · 日期:2026-06-11
> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:[`0001-positioning-vs-nixos.md`](0001-positioning-vs-nixos.md)(本 ADR 触发其预设演进)、Foundation [`../../layers/01-foundation.md`](../../layers/01-foundation.md)
> 前提:用户态引擎已成熟——8 个里程碑端到端验证(store/世代/求解/补闭包/install/全裸容器),见 [`../../CHANGELOG.md`](../../CHANGELOG.md)

---

## 背景

ADR-0001 把 Aevum 定位为"用户态系统层,复用宿主内核,第一阶段不啃 bootloader/内核"。但它**明确预留了演进口子**:

> (ADR-0001 §理由)"可演进:用户态系统层成熟后,未来要做成可引导发行版(打包内核 + Aevum 用户态)是**增量工作**。"
> (ADR-0001 §后续可重新审视)"若用户态层成熟且有强需求,可启动'可引导发行版'增量项目(打包内核)。"

**触发条件现已满足**:
- 用户态层成熟:里程碑1-8 验证了 store/世代/原子切换/回滚/GC/求解/install/全裸运行闭包。
- 强需求:用户明确要"直接当自己的 Linux 发行版用"。

本 ADR 据此**启动可引导发行版增量项目**,并钉死它的边界、与现有引擎的关系、分阶段路线。

---

## 决策

**启动 `bootable-aevum` 增量项目:在现有用户态引擎之上加"可引导"层,使 Aevum 世代能作为真实系统根被内核引导起来。**

核心原则(守住 Aevum 的根):

1. **引擎不变,只加外壳**。store/世代/求解/install 等已验证引擎**零改动**;可引导是在其外包一层"内核 + initramfs + init 接管"。
2. **内核仍不自造**(守 ADR-0001 精神):打包**上游内核**(Debian/通用 bzImage),不重写内核、不做驱动。差异化仍在"AI 维护的可复现世代",不在内核。
3. **世代延伸到引导**:一个"可引导世代"= 用户态闭包 + 该世代用的内核 + initramfs。激活含新内核的世代需重启(这是与用户态 symlink 切换的本质区别,诚实标注)。
4. **增量、可回退**:每阶段独立可验证;任何阶段失败,用户态引擎照常工作(仍能在宿主 Linux 上当包管理器用)。

---

## 与 Nix 的对照(为什么这不是"变成另一个 Nix")

| | NixOS | bootable-aevum |
|---|---|---|
| 内核 | 上游内核,Nix 管 | 上游内核,Aevum 管 |
| 引导 | 改 GRUB/systemd-boot 菜单 | 同类机制(写 bootloader 项) |
| 世代含内核 | 是 | 是(本 ADR 引入) |
| 意图层 | Nix 语言 | AI + 模板(Aevum 差异化,不变) |
| 求解/可复现 | Nix 求解 | 确定性求解器 + lock(已验证,不变) |

→ 引导机制向 Nix 靠拢(本就是成熟范式),但**意图层 + AI 维护 + 内容寻址引擎是 Aevum 自己的**,差异化未丢。

---

## 分阶段路线(每阶段独立可验证)

### 阶段 0:环境就绪(准备)
- QEMU(已装,scoop qemu 11.0.0)。
- 内核 bzImage:用 Aevum 自己的 install 从 Debian 下 `linux-image-*` 包,取出 vmlinuz(吃自己狗粮)。
- busybox-static:做最小 init/shell(静态,不依赖任何库——呼应 Foundation 的 min-toolset 静态链接原则)。

### 阶段 1:最小可引导(第一块地基)★ A 路线第一步
- 目标:QEMU 里,内核 + initramfs 启动 → init 从 **Aevum store/世代** 拉起 `/bin/sh`(或 hello),进到一个 shell。
- 做法:
  - initramfs 内放一个极简 init(busybox 或自写),它**挂载/切到 Aevum 世代目录作根**,exec 世代里的 shell。
  - 复用里程碑8 的 export-rootfs:把"世代闭包"导出成 initramfs 内容。
- 成功标志:QEMU 启动后落到一个能跑命令的 shell,且这个 shell 来自 Aevum store(非 initramfs 自带)。
- 验证 Aevum 世代能当真实系统根。

### 阶段 2:init 接管 + 世代切换
- 真 init(进程1)接管,读 Aevum active 世代,挂载世代为根。
- 世代切换 = 重建 initramfs/bootloader 项指向新世代内核,重启生效(对照用户态的 symlink 秒切)。

### 阶段 3:bootloader + 多世代菜单 + 回滚
- 写 bootloader(extlinux/systemd-boot)菜单,每个可引导世代一项;开机选世代 = 选内核+用户态闭包;回滚 = 选旧项重启。
- 对标 NixOS 的"开机菜单选世代"。

### 阶段 4+:系统配置/服务/网络(发行版的长尾)
- init 系统(服务管理)、用户/网络/fstab/locale 等系统配置。
- maintainer scripts 执行(postinst)。
- 这是"发行版长尾",量大,按需推进。

---

## 边界(诚实划定)

**在 bootable-aevum 范围内:**
- 打包上游内核 + initramfs + init 接管
- 世代延伸到引导层(含内核选择)
- bootloader 菜单 + 多世代引导/回滚
- QEMU 中验证(不碰真实物理机引导,除非用户另要)

**不在范围内(守 ADR-0001 精神):**
- 重写内核 / 写驱动 / 内核态模块
- 硬件适配深渊(交给上游内核)
- 第一阶段不上真实物理机(QEMU 足够验证机制)

---

## 风险与现实(逐条标注)

1. **环境门槛远高于里程碑1-8**:需 QEMU + 内核镜像 + initramfs + init,验证从"WSL 跑测试"变成"QEMU 引导整机"。下载量大、调试慢。
2. **世代切换语义变了**:含内核的世代切换需重启,不再是亚毫秒 symlink。回滚也要重启。PoC-7 验证的用户态原子切换在引导层不适用,需新机制。
3. **init 是进程1**:崩了系统就崩,比用户态任何 bug 严重。Foundation 的 min-toolset 静态链接原则在此是救命的(兜底 shell 不依赖任何东西)。
4. **工程量级跳变**:这是"从包管理器到发行版"的根本转向,阶段4+ 是数月级长尾,不应一次做完。每阶段独立交付。
5. **不放弃用户态价值**:任何阶段失败,Aevum 仍是能在任意 Linux 上用的包管理器(里程碑1-8 成果不依赖本 ADR)。

---

## 影响

- 新增 `docs/architecture/bootable/` 子目录(本 ADR 落地后写各阶段详细设计)。
- 用户态引擎文档不变(本 ADR 是增量,不修改既有定位文档,只触发 ADR-0001 预设演进)。
- 实现遵循项目方法论:每阶段先设计、再 QEMU 实证、再下一阶段。

---

## 后续

- 阶段1 是 A 路线第一块可验证地基;其设计细节(initramfs 布局、init 如何挂载世代根)待本 ADR 接受后单独成文。
- 内核打包复用 Aevum install(从上游下 linux-image),是"Aevum 管自己的内核"的第一步实证。
