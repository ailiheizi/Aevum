# 可引导发行版(bootable)— 各阶段设计

> 父文档:[`../adr/0006-bootable-distribution.md`](../adr/0006-bootable-distribution.md)
>
> ADR-0006 把 Aevum 从"用户态包管理器"增量演进为"可引导发行版"(路线A),分阶段、每阶段独立 QEMU 可验证。本目录存各阶段详细设计。

## 阶段状态总览

| 阶段 | 内容 | 状态 | 凭据 |
|---|---|---|---|
| 0 | 环境就绪(QEMU、内核、busybox-static) | ✅ 已完成 | CHANGELOG,`scripts/` |
| 1 | 最小可引导:内核 + initramfs → 从世代拉起 shell | ✅ 已完成 | CHANGELOG 阶段1 |
| 2 | init 接管 + switch_root 切世代为真实根 | ✅ 已完成 | CHANGELOG 阶段2 |
| 补真 | 让引擎(generation/store)驱动 bootroot,非脚本手拼 | ✅ 已完成 | CHANGELOG 第22轮 |
| 收口 | install 补运行闭包入世代,世代真正自包含 | ✅ 已完成 | CHANGELOG 第23轮 |
| 3 | bootloader 多世代菜单 + 命令式回滚 | ✅ 已完成 | CHANGELOG 第24/25轮,`crates/generation/src/bootloader.rs` |
| **4** | **真 init(s6) + 服务管理 + 系统配置** | **📝 设计草案** | [`04-init-services-config.md`](04-init-services-config.md) |

> 阶段1-3 在实现期是直接在 CHANGELOG + 代码/脚本里推进、未单独成文(项目早期"先实证后补档"的节奏)。本目录从阶段4 起为每阶段先成文设计、再实证。阶段1-3 的实证细节见 CHANGELOG 对应轮次与 `scripts/build-*.sh`、`crates/generation/src/bootloader.rs`。

## 文档

- [`04-init-services-config.md`](04-init-services-config.md) — 阶段4 设计草案(init 路线 s6/s6-rc、服务 TOML schema、/etc 基底+overlay、子阶段拆分 4a-4e)。**待评审,未进入实现。**

## 不可违反的边界(贯穿所有阶段,来自 ADR-0006)

1. 引擎不变,只加外壳:store/世代/求解/install 零改动。
2. 增量、可回退:任何阶段失败,Aevum 仍是能在宿主 Linux 上用的包管理器。
3. 世代延伸到引导:可引导世代 = 用户态闭包 + 内核 + initramfs。
4. 诚实标注简化:QEMU 非物理机、上游共享内核等,都在各文档/CHANGELOG 标注。
