# 文档变更日志

本文件记录 Aevum 设计文档的演进。遵循"每次有意义的设计变更都留痕"的原则。

---

## 2026-06-27(六)—— P1 批次:CI 上线 + 健壮性/可用性修复

承接 P0 全清,本轮做 P1 高价值项。**最大成果:CI 第一次真跑 unix 测试**。

### P1-1 GitHub Actions CI(ubuntu 真跑 unix 测试)

- 全仓此前零 CI:所有 `#[cfg(unix)]` 安全测试(setuid/symlink/原子切换/dlopen 闭包)在作者的 Windows 机上被 `cargo test` 静默跳过 —— unix 回归可绿着合入。
- 加 `.github/workflows/ci.yml`:ubuntu-latest、钉 Rust 1.88、装 binutils/xz,跑 `cargo build + test --workspace --locked`(阻断门);fmt+clippy 为 advisory(`continue-on-error`,待格式校准后转阻断)。
- **上线即抓到 2 个真问题**:(1) 我自己写的 `active_lock_pointer` 测试依赖 mtime tie-break,在快速 ext4 上 flaky(Windows 计时器较粗才偶过)→ 删掉非契约的 mtime 断言;(2) `boa_engine` 实际要 rustc 1.88,而 `rust-version` 谎称 1.85 → 修正 MSRV。迭代 3 次后 build-test 稳定绿。

### P1-2 clean-clone / CI 编译(vendor config)

- 提交的 `.cargo/config.toml` 强制走被 gitignore 的 `vendor/`,任何 clean clone / CI 直接编译失败。
- 改:untrack `.cargo/config.toml`(本地保留,作者离线 WSL 构建不受影响)+ gitignore;提交 `.cargo/config.offline.toml` 作 opt-in 模板;clean clone 默认走 crates.io。

### P1-3 `aevum update` 不再毁索引

- 旧 `sh -c "curl | gunzip > index"`:管道退出码只看 gunzip(curl 失败被吞),`>` 在下载失败前已把索引截断成空。
- 改:argv 形式 curl `--fail` | gunzip → 临时文件,两端成功且非空才原子 rename。e2e:404 镜像 → 旧索引完好保留。

### P1-4 `aevum init`(修开箱即崩)

- README quick-start 让用户 `update` 后立刻 `source env.sh`,但 env.sh 只在首装后才写 → 首次 source `No such file or directory`。
- 加 `aevum init [--update]`:建 root/profile/{bin,lib}/locks/generations + 写引导 env.sh(首装前可 source)。README 以它开篇。e2e 验证 init 后立刻 source 成功。

### P1-6 NAR 中断恢复(原子提交)

- `fetch_and_unpack` 原地解进 dest,仅 xz 非零才清理。SIGKILL/断电/磁盘满 留半成品 dest,下次 `exists()` 当完整 → 损坏对象入世代。
- 改:解进 `.tmp-<pid>-<ref>`,xz 成功 + NarHash 校验通过才原子 rename 到 dest;dest 只在提交点出现且必完整;残留临时目录进函数即清。e2e:真拉 ripgrep,dest 出现、零残留临时目录。

### P1-5 并发锁(flock)

- 全仓无 advisory lock:两个并发 install 都取 `next_generation_id`=max+1 算出**同一** id,交错写同一 `gen-NNN/packages`,还互相 `remove_dir_all` 共享的 unpacked 目录。
- 加 `FsLock`:对 `$AEVUM_ROOT/.lock` `flock(LOCK_EX)`,RAII(进程退出/崩溃内核自动释放);main() 在 match 前对变更类子命令(install/remove/update/maintain/nix-fetch/switch/rollback/gc/activate/compose/ai)取锁,只读命令不取可并发;gen id 在锁内分配。仅 libc(已 vendored)。
- e2e:shell `flock` 持锁 4s,期间真 `aevum gc` 被阻塞满 4s(而非立即返回),证明 Rust FsLock 与 OS flock 互锁。

### P1-7 make_generation 原子构建

- `make_generation` 原地建 `packages/`+`lock.txt`,中断留半填充 gen-NNN 被当完整世代用(缺文件、GC 漏算)。
- 改:建进 `.gen-NNN.tmp.<pid>`,完整后原子 rename 到 gen-NNN;失败清临时目录;重建先删旧再 rename。临时名被三处 gen-NNN 枚举器安全忽略。后续 templates/source-lock 写入在 rename 之后,时序不变。
- 测试:+1 rebuild 正确替换 + 零残留临时目录 + lock.txt/packages 同时就绪。

### 其它

- 修正 workspace `repository` URL(`aevum/aevum` → `ailiheizi/Aevum`)。

### 仍待(P1 余项,见 AUDIT-ROADMAP.md)

- fmt/clippy 转阻断(本环境装不上 rustfmt/clippy 组件,待本地 `cargo fmt` 一次后由 CI 去掉 `continue-on-error`)、P0-2 签名半边(需 vendor ed25519)。

---

## 2026-06-26(五)補2 —— P0-2 NAR 完整性校验(零新依赖,真包 e2e 验证)

闭合上一轮"未决"里 P0-2 的**完整性**半边:NAR 下载现在校验内容哈希。修复前 `curl|xz|unpack` 无条件信任镜像字节,`NarHash`/`FileHash` 解析了却从不用——传输损坏 / 中间人 / 镜像投毒全检测不到。

### 实现(零新依赖,offline-safe)

- 新增 `crates/nix-source/src/nix_base32.rs`:Nix **自定义** base32 编解码(字母表 `0123456789abcdfghijklmnpqrsvwxyz`,去 e/o/u/t;位反序,同 Nix `printHash32`)。纯函数,round-trip 测试自证位序正确。
- `NarInfo::verify_nar_hash()`:解析 `NarHash: sha256:<nixbase32>`,与计算摘要比对。仅 sha256;缺失/其它算法**硬报错**,绝不静默放行。
- `fetch_and_unpack`:`HashingReader` tee NAR 流,边解包边算 SHA256,drain 到 EOF(NarHash 覆盖整个流,unpack 可能不读完),再校验。不符 → 删解包结果 + 报错。不缓存整个 NAR。
- 依赖只用 `sha2`+`hex`(store 早在用,已 vendored)——**无需重新 vendor**。

### 验证

- 真实 e2e(USTC Nix 镜像):`nix-fetch --resolve ripgrep` 拉 ripgrep-15.1.0 + 7 依赖共 8 个 NAR,完整性校验全过真包、不死锁、不误报。
- +8 单测:nixbase32(round-trip / 已知长度 / 字母表)、verify_nar_hash(接受 / 篡改拒绝 / 缺失 / 坏算法)。nix-source 19 测全过。

### 边界(诚实声明)

完整性校验**击穿传输损坏与未察觉的内容篡改**,但挡不住"完全恶意、会同时伪造 NarHash 的镜像"——那需要 **ed25519 签名校验**(验 narinfo `Sig` 对 trusted-public-keys)。签名半边需引入 `ed25519-dalek` 并重新 vendor(离线构建约束),留作独立任务。

---

## 2026-06-26(五)補 —— 8 维度代码审计 + 5 项安全/正确性修复 + 双语 README

用多 agent workflow(8 维度并行审计 → 每条发现 2 重对抗验证 → 综合)对 v0.1.0 全代码库做了一次穷尽审计:**57 条发现**通过验证,产出 [`AUDIT-ROADMAP.md`](../AUDIT-ROADMAP.md) 的 P0/P1/P2 优先级路线图。本轮按路线图修掉 P0 七条里的六条。

### 安全修复(security)

- **P0-1 NAR 路径穿越 → 任意文件写**(critical):`fetch_and_unpack` 用下载来的 `StorePath`(不可信)直接 `store_dir.join(..)` 落盘。恶意/被劫持镜像给 `StorePath: /etc/cron.d/evil`(无 `/nix/store/` 前缀)→ Unix 下 `Path::join` 丢弃 base 写绝对路径;`/nix/store/../../x` → `..` 上跳。把任意不可信镜像变成任意写。
  修复:新增 `NarInfo::validated_ref_name()`,强制 `/nix/store/` 前缀 + 拒绝 `/`/`\`/NUL/`.`/`..`/空/非 `<32 hash>-<name>` 形状。`crates/nix-source/src/{cache.rs:72,narinfo.rs}`。
- **P0-3 `resolve_package` shell 注入 → 潜在 RCE**(high):旧实现 `sh -c "curl '{url}' | xz -d | grep -F -- '-{name}'"`,单引号即逃逸。AI dispatch 层已在产出包名字符串,接入即 RCE。
  修复:弃 `sh -c`,argv 形式 spawn curl/xz + 管道,匹配在 Rust 里做;`name` 仅作数据。`crates/nix-source/src/cache.rs`。

### 正确性 / ADR 修复

- **P0-5 / P0-6 `list`/`remove` 认 active 世代**(high,回滚可信):二者旧实现取 `locks/` 里 mtime 最新的 lock 判断"当前装了什么"。回滚后 active 是旧世代、最新 lock 是后装的 → `list` 谎报包集,`remove` 从错误基线重建(可能复活已回滚掉的包)。
  修复:install/remove 写 `gen-<id>/source-lock.txt` 记录世代由哪个 lock 构建;新增 `active_lock_name()` 解析 **active 世代 → 它的 lock**,旧世代无指针时回退 mtime-latest。`crates/cli/src/{lib.rs,main.rs}`。
  e2e:装 busybox(gen-1)→ 装 ripgrep(gen-2)→ `list` 显 ripgrep → `rollback 1` → `list` 正确显 busybox(修复前仍显 ripgrep)。
- **P0-4 AI install 走 verify 门禁**(high,ADR-0005):`aevum ai "装..."` 旧路径 `do_install→install→set_active` 完全跳过门禁。违反 ADR-0005(AI 参与选包则世代须由 verify machine 独立复核,AI 不能自我放行)。
  修复:新增 `install_gated()`(propose → `activate_verified` 门禁);`do_install` 加 `gated` 标志,AI 路径传 true、裸 `install` 命令传 false。硬失败(完整性/闭合)永不可绕过,版本回退需 confirm(`--yes` 兼作)。`crates/cli/src/{lib.rs,main.rs}`。
  e2e:`aevum ai --yes "装个 busybox"` → `🛡 verify 门禁通过` + 写 `gen-001/verified`(passed=true,三判据全 0)。

### 文档修复

- **P0-7 `gc --keep` 文档数据丢失 bug**(导致误删):`--keep` 是要保留的世代 **id 列表**,不是"保留最近 N 个"。`aevum gc --keep 3` 只留 gen#3 删其余。两个 README 已改 `--keep 1,2,3`。
- **`export-system --generation` 文档 bug**:实为位置参数(`crates/cli/src/main.rs:177`),旧写法 parse 失败。改 `aevum export-system 1 --out ...`。

### 文档:英文 README 设为默认

- `README.md` 改英文(国际/开源受众默认),新增 `README.zh-CN.md` 中文版,顶部互相切换。
- 两版命令表均对着真实 clap `Command` enum 核过;补 WSL 编译说明、`aevum --help` 指向进阶命令。

### 回归测试

- `narinfo.rs` +3(合法接受、7 穿越向量 + 2 坏形状拒绝);`active_lock_pointer.rs` +3(回滚跟随 / 无 active 返回 None / 旧世代回退)。
- nix-source 11 测、cli verify_gate 3 + active_lock 7,全 workspace lib 套件全绿。

### 未决

- **P0-2 NAR 完整性/签名校验**:~~完整性已在補2 完成(NarHash 校验,零新依赖)~~;**签名**半边(ed25519 验 `Sig`)仍待——需引入 `ed25519-dalek` 并重新 vendor(离线构建约束),单独做。
- P1 大项:加 CI(目前 unix 测试在 CI 缺位下从未自动跑)、`.cargo/config.toml` 致 clean clone 编译失败、`aevum update` 吞 curl 失败、无并发锁等,见 `AUDIT-ROADMAP.md`。

---

## 2026-06-26(五)—— 统一 `aevum ai` 入口 + AI 修依赖冲突端到端验证

把分散的 AI 能力(意图翻译 / explain / 冲突 repair)收敛成**一个命令** `aevum ai "<话>"`,
并用真实 DeepSeek 对 4 个依赖冲突场景做了端到端验证。

### `aevum ai`:一个命令处理所有

- AI 一次调用判断意图(install / explain / repair / search / list / gc / chat)+ 给回复,CLI 据此分发。
- 多轮对话:历史存 `$AEVUM_ROOT/ai-history.txt`(role\tcontent,换行转义,保留最近 20 条),跨命令接续。
- `--reset` 清空历史;`--yes` 跳过确认。
- 有副作用的动作(install/remove/gc)默认要确认,只读的(explain/search/list/chat)直接执行。
- AI 不可用时报错提示配置,底层命令(install 等)仍可直接用;意图翻译降级到离线 Mock。

### AI 修依赖冲突 —— 4 场景真实验证(DeepSeek)

| 场景 | 冲突构造 | 求解器可行解 | AI 决策 | 对 |
|------|---------|------------|--------|----|
| 1 | libssl `>=3.0` + `<=3.2` | 有交集,无冲突 | 不误报 | ✅ |
| 2 | libfoo `=1.0` vs `=2.0` | 无交集,A 失效 | 方案C 保留两份 | ✅ |
| 3 | libhttp 贪心选 3.0 违反 `<=2.0` | A 可解(钉 2.0) | 方案A 放宽(风险最低) | ✅ |
| 4 | libcore `=1/=2/=3` 三方互斥 | C 也救不全 | 方案D 交用户决策 | ✅ |

**关键观察**:AI 的推荐**跟着确定性求解器算出的可行方案走**,不是瞎猜——能放宽就 A,不能就 C,
连两份都救不了就老实交 D。守住 ADR-0003 红线(AI 选方案/给理由,确定性求解器算具体版本);C/D 标"需人工确认"。

### 回归测试(离线、不依赖网络/key)

- 把解析逻辑抽成纯函数 `parse_dispatch_response` / `parse_repair_response`(从 `ai_dispatch` / `ai_evaluate_repair` 提取)。
- 用上面 4 个场景真实抓到的 AI 响应文本固化成 9 个新测试:dispatch 的 install/explain/list/search 意图 + 畸形响应降级 chat;repair 的 A/C/D 方案 + 畸形响应回落 A。
- **安全兜底**:AI 不按格式回复时,意图降级 chat、修复降级 A 占位,绝不误触发安装/卸载。
- intent crate 22 测全过;全 workspace lib 套件 158 测 0 失败。

### 涉及文件

- `crates/intent/src/ai_client.rs`:`ChatHistory` / `ai_dispatch` / `ai_chat_history` / `parse_dispatch_response` / `parse_repair_response` + 9 测试
- `crates/cli/src/main.rs`:`Command::Ai`(分发 handler)+ 提取 `do_install` 共享核心
- `README.md`:主推 `aevum ai` 为单一 AI 入口
- commit:`72771ae`(统一入口)、`2eb0bd9`(解析回归测试)

### 局限

- 端到端的真实 AI 链路需要 API key(验证用的 key 未入库);无 key 时只跑离线解析回归。
- 意图判断依赖 prompt 工程,极端模糊输入可能误判(已用畸形响应降级 chat 兜底,不会误执行副作用)。

---

## 2026-06-23(一)—— 第五十六轮补充:QEMU 引导验证("Aevum 发行版"在虚拟机里启动)

在第五十六轮 export-system 基础上,验证了完整的"发行版引导"链路:

### 证据(boot.log)

```
[    4.297157] Run /bin/sh as init process

BusyBox v1.37.0 (Debian 1:1.37.0-6+b8) built-in shell (ash)
Enter 'help' for a list of built-in commands.

~ #
```

### 链路

```
Linux 内核 6.12.86(来自 Debian 索引,Aevum unpacked/)
  + initramfs(Aevum export-system 从 gen-100 世代打包)
    + busybox-static(Aevum store 里的真实 Debian 包)
→ QEMU(-kernel + -initrd + rdinit=/bin/sh console=ttyS0)
→ 启动到 BusyBox shell 提示符
```

### 脚本/文件

- `vm/start-vm.ps1`:一键启动 Debian cloud image VM(SSH 进入)
- `vm/vmlinuz`:Debian 6.12 内核
- `vm/initramfs.cpio.gz`:Aevum 世代打包的 initramfs(含 busybox)
- QEMU 命令(手动跑可交互):
  ```
  qemu-system-x86_64 -accel tcg -m 512 -kernel vm/vmlinuz \
    -initrd vm/initramfs.cpio.gz \
    -append "rdinit=/bin/sh console=ttyS0" -nographic
  ```

### 局限

- TCG(纯软件模拟)慢;WHPX 加速下 guest 网络未通(需进一步调试)。
- initramfs 只含 busybox(最小 shell);完整桌面需要完善 Debian 基座方案。
- Claude Code 的 PowerShell 环境 ReadLine 阻塞无法完成交互(shell 提示符不带换行);手动终端里可直接交互。



路线1 里程碑:`aevum switch <gen>` → `$AEVUM_ROOT/profile/bin` 自动刷新 → 用户 PATH 里的程序立即换版。跟 Nix 的 `~/.nix-profile/bin` 完全同一个模式:稳定路径 + 内容随世代切换而变。

### 端到端验证

```bash
$ aevum switch 100
[switch] active → gen-100
  profile/bin: 1 个可执行文件已就绪

$ export PATH="$AEVUM_ROOT/profile/bin:$PATH"
$ busybox echo "ROUTE-1-WORKS"
ROUTE-1-WORKS
```

用户一次性把 `$AEVUM_ROOT/profile/bin` 加进 `.bashrc` 的 PATH,之后每次 `switch`/`rollback` 世代都自动生效——无需重登、无需手动更新 symlink。

### 改动

- `cli` lib:`Layout::profile_bin_dir()`(`$AEVUM_ROOT/profile/bin`)。
- `cli` lib:`refresh_profile(layout)`:扫描 active 世代的 `generation_refs`,对 `usr/bin/`、`bin/`、`sbin/`、`usr/sbin/` 下的条目,在 `profile/bin/` 建 `<name>` → `<store_dir>/<name>` 的 symlink(指向 store 内实际 ELF 文件)。原子更新(新建临时目录 → rename 覆盖旧 profile/bin)。
- `cli` main:`Command::Switch` 与 `Command::Rollback` 在 `set_active` / `rollback` 后自动调 `refresh_profile`。失败不阻塞(打印 warning)。
- 修复 `refresh_profile` 的 active 解析:兼容 `set_active` 写的相对路径(`./.aevum/generations/gen-NNN`),从路径组件中提取 `gen-NNN` 段。

### 设计对齐(跟 Nix 同模式)

| Nix | Aevum |
|---|---|
| `~/.nix-profile/bin/` | `$AEVUM_ROOT/profile/bin/` |
| `nix-env --switch-generation N` | `aevum switch N` |
| profile → current generation | profile/bin symlinks → active 世代 store 文件 |
| 内容随世代变,路径不变 | 同 |

### 验收

- 端到端:WSL 真跑 `switch 100` → profile/bin/busybox 自动生成 → PATH 里 `busybox echo "ROUTE-1-WORKS"` 输出成功。
- 全 workspace lib 单测全绿(--offline)。

### 边界与待办

- `refresh_profile` 只处理 `*bin*/*sbin*` 路径下的条目;库文件(`lib/`)/数据文件不建 symlink(那是另一层:LD_LIBRARY_PATH 或 linker 配置)。
- 世代 store 对象布局是"目录包文件"(每个 rel_path → store **目录**内的同名文件);profile/bin symlink 正确指向目录内文件(不是指向目录)。
- `set_active` 写的相对路径有 bug(`./.aevum/generations/gen-N` 从 symlink 所在目录解析不到),refresh_profile 已兼容;但 `active` symlink 的 `exists()` 返回 false(dangling)。建议后续修 `set_active` 写纯相对名(`gen-NNN`)。



Aevum 从"包管理器 demo"到"可运行系统"的关键跨越:从世代导出完整 rootfs,其中的二进制**真的能执行**。

### 里程碑达成证据

```
$ /tmp/aevum-system/usr/bin/busybox echo "hello-from-aevum-system"
hello-from-aevum-system

$ /tmp/aevum-system/usr/bin/busybox uname -a
Linux User 6.6.87.2-microsoft-standard-WSL2 #1 SMP ...
```

Aevum 管理的世代(store 对象 → generation → export_system)产出的 rootfs 里,busybox-static(2MB 静态 ELF)直接执行成功。这是 chroot/systemd-nspawn 进去后用户会体验到的同样结果。

### 改动

- `cli` lib:新增 `export_system(layout, gen_id, dest) -> ExportSystemReport`:在 `export_bootroot`(铺世代文件树)基础上补齐 nspawn/chroot 所需最小骨架:
  - `/etc/passwd`(root+nobody)、`/etc/group`、`/etc/hostname`、`/etc/resolv.conf`
  - `/bin/sh` 软链 → 世代内的 busybox/busybox-static(自动搜索候选列表,含根目录扁平文件)
  - `/root` home 目录
- `cli` main:新增 `Command::ExportSystem { generation, out }`,打印 nspawn/chroot 命令示例。
- 修复 `find_and_link_shell`:加入根目录候选(部分世代 rel_path 为扁平文件名)。

### 验收

- `cli/tests/export_system.rs` 2 测:含 busybox 合成世代导出正确骨架(/bin/sh 软链 + /etc + 基础目录)、无 shell 时 shell_found=false。
- **WSL 真实执行验证**:手建含真实 busybox-static ELF 的 gen-100 → export_system → `/tmp/aevum-system/usr/bin/busybox echo "hello-from-aevum-system"` → 输出成功。
- 全 workspace lib 单测全绿(--offline)。

### 边界与待办(诚实记录)

- **chroot 需 root 权限**:WSL2 环境 `sudo chroot` 因无 TTY 卡住;**二进制可执行性已证明**(跟 chroot 内跑是同一个二进制同一个内核,无差异)。有 root 的真机/容器里 `sudo chroot <rootfs> /bin/sh` 即可进入完整 shell。
- **世代质量依赖正确的 ingest**:gen-010(早期轮次)store 对象含元数据垃圾(lintian override);gen-100(本轮手建)用正确 rel_path 和真实 ELF 即正常。maintain 主循环的 propose 已按 rel_path 正确入库。
- **busybox applet symlinks 未做**(只建了 /bin/sh;可选 `busybox --install` 建全套 ls/cat/mount 等)。
- **s6 服务启动未做**(路线 3 本轮只证 shell 能跑)。

### 路线 3 完成状态

| 目标 | 状态 |
|---|---|
| export 完整 rootfs | ✅ export_system 命令 |
| /bin/sh 可用 | ✅ 自动检测 busybox 建软链 |
| 二进制真能执行 | ✅ busybox echo + uname 成功 |
| chroot/nspawn 进入 | ⚠ 需 root 权限(命令已给出) |



承第四十九~五十四轮:TS 前端 `aevum resolve --config` 能产 lock,但 `aevum maintain`(主循环:resolve→propose→verify→activate)此前只支持显式包名和 --intent,不支持 --config——TS 前端无法一键跑完全链路装系统。本轮接通。

### 改动

- `Command::Maintain` 新增 `--config <ts>` 与 `--inputs <json>` 参数(与 --intent/包名三选一)。
- `maintain_cmd`(unix + non-unix 两版):新增 config 分支——读 TS → `ts_config_to_constraints`(共享逻辑,含模板展开)→ 摊开约束让用户确认(--yes 跳过)→ `resolve_constraints_opt`(写 lock,带 inputs/templates 记录)→ `maintain_from_lock`(propose→verify→activate)。
- 既有的 intent/显式包名路径不变;else 分支错误提示更新为"需要包名、--intent 或 --config"。

### 语义(ADR-0003/0004 边界对齐)

TS 前端(AI+模板+可编程组合)只在 lock **之前**介入——产出约束、写 lock 是 synth 阶段;lock 之后的 propose/verify/activate **全程无 AI/无 TS**(可复现只来自 lock)。maintain --config 不改变这个边界,只是把 resolve --config 的入口串进了主循环。

### 验收

- CLI 编译通过(--offline,离线 vendored 构建)。
- 全 workspace lib 134 单测 + 23 集成测试(audit/template/consistency/ts_inputs/repair)全绿,**--offline 跑**——同时验证 maintain 签名改动无回归 + 离线构建仍正常。
- 端到端暂未做真实 propose(需镜像下载真实 .deb);但 resolve→lock 段复用已验证的 ts_config_to_constraints + resolve_constraints_opt,正确性已由既有集成测试覆盖。

### 边界与待办(诚实记录)

- **maintain --config 的端到端(真实 propose+activate)未做**(需联网镜像下载 .deb + unix 环境;maintain_loop 测试原就跳过网络不可达情况)。resolve→lock 段已充分测试。
- **三选一未强制互斥**:同时传 --config + --intent + 包名不会报错(按 config>intent>packages 优先级静默忽略后者)。可后续加互斥校验。
- 仍未做(承前):相对 import 文件加载、复杂 semver range。



承第四十九轮引入 boa(TS 前端)后,`.cargo/config.toml` 一直指 ustc 在线镜像,`vendor/` 那 43 个旧包成了不被引用的死目录——离线可复现构建能力丢失。本轮收尾:`cargo vendor` 重生成 vendor/,切回离线 vendored-sources。

### 动手前的并行调研(workflow)

用一个 understand workflow 并行摸清四件易踩的事:当前状态(确认 build 走 ustc、vendor/43 死包、Cargo.lock 已锁 boa 全家)、cargo vendor 陷阱(icu 数据 crate 体积)、离线验证策略(`--offline` 判据)、回滚方案(备份 config)。据此定执行步骤。

### 改动

- 备份 `.cargo/config.toml` → `.cargo/config.toml.bak`(回滚点;原指 ustc 在线镜像)。
- `cargo vendor vendor` 重生成 `vendor/`:**43 → 167 包**(含 boa 全家 7 个 0.21.1 + icu 国际化库)。
- `.cargo/config.toml` 切到离线:`[source.crates-io] replace-with="vendored-sources"` + `[source.vendored-sources] directory="vendor"`。

### 验收

- `cargo build --workspace --offline` + `cargo test --workspace --lib --offline`:**无 Downloading/Updating 行**(确证只读 vendor/、不触网),全绿。
- boa 全家在 vendor/ 内,TS 前端离线可编译运行。

### 边界与待办(诚实记录)

- **vendor/ 体积增大**:167 包(含 icu 数据);这是 boa 的固有代价,已接受。
- vendor/ 非 git 跟踪(项目非 git repo);config.bak 留作回滚。
- 离线可复现指 **Rust 依赖**层面;Debian 包索引仍需 prep-index.sh 在线拉取(数据层,与构建可复现分属两事)。
- 仍未做(承前):相对 import 文件加载、复杂 semver range。



承第四十九~五十二轮:TS 前端 + inputs 入 lock + 模板系统都已落地。本轮补上"反向验证":新命令 `aevum audit-config <ts> --against <lock>` 用 lock 记录的 inputs 重跑源 TS 配置 → 重新求解 → 比对 closure_id,检测源配置是否漂移。

### 厘清:这不是"重放求可复现"

可复现**已经只来自 lock**(closure_id + 锁定包集),重放历史世代直接用 lock。本命令是**旁路审计/漂移检测**:确认某历史 lock 是否仍能由其源 TS 配置 + 记录的 inputs 重新产出。一致=源未漂移;不一致=源 .ts/模板被改,或索引快照变化。

### 改动

- `cli` lib:
  - `read_lock_ts_inputs(path)`:读回 lock 头部 `ts_inputs:` 行(补齐 closure_id/templates 之外的最后一个头部读回)。
  - `ts_config_to_constraints(layout, ts, inputs)`:抽出"沙箱求值 → 模板展开+合并"的共享逻辑(resolve --config 与 audit 共用,消除重复)。
  - `audit_config(layout, ts, against, inputs_override) -> AuditReport`:读 lock 的 closure_id + ts_inputs,用记录的 inputs(或 `--inputs` 覆盖)重跑求解(写临时 lock `audit-<name>`),比对 closure_id。
  - `AuditReport`:drifted / 期望与实际 closure_id / 包数 / 重放输入,带可读 Display。
- `cli` main:`resolve --config` 分支改调 `ts_config_to_constraints`(行为不变的小重构);新增 `Command::AuditConfig`,漂移时非零退出码(CI 友好)。

### 验收

- `cli/tests/audit_config.rs` 5 测:同源同输入未漂移、改源(多 use 一包)漂移、`--inputs` 覆盖改变结果、`read_lock_ts_inputs` 往返(有值/none)、缺 lock 报错。
- 既有 resolve --config / 模板 / 一致性回归未受重构影响。
- 全 workspace lib 单测全绿,0 warning。

### 边界与待办(诚实记录)

- **漂移成因不区分**:closure_id 变化可能来自源 .ts/模板改动 **或** 包索引快照变化;报告已提示两种可能,但不自动判别是哪一种。
- **audit 写临时 lock**(`audit-<name>`)占 locks 目录,比对后保留供调试(像普通 lock)。
- **inputs 单行化有损**(第五十轮):重放用的是 lock 里单行化后的值;原始 inputs 含换行时与原值略有差异(JSON 单参数通常单行,极低概率)。
- 仍未做(承前):相对 import 文件加载、复杂 semver range、恢复离线 vendor、包级深度 diff 呈现(本轮只到 closure_id + package_count)。



承第五十一轮:模板系统落地,但"世代记录所用模板"(验收7)只提供了 `record_generation_templates` 函数,未接入主循环。本轮把它串进 `resolve → lock → maintain → 世代` 数据流,世代构建时自动记录所用模板。

### 数据流(经调研锁定接入点)

`resolve --config` 阶段还没有世代(世代在 maintain/install 才建),故模板记录走 **lock 文件**作桥:
1. `resolve --config` 展开模板 → `collect_templates` 收集模板+版本(含 extends 链)→ 序列化进 lock 头部 `templates:` 行(同 ts_inputs 性质,纯审计、不进 closure_id)。
2. `maintain_from_lock` 在 `propose_generation` 建好世代后,`read_lock_templates` 读回 → `record_generation_templates` 写 `gen-NNN/templates.txt`。

### 改动

- `template`:新增 `collect_templates(dir, roots) -> Vec<(name, version)>`,DFS 收集 roots 及其 extends 传递闭包的模板版本(复用无环校验,去重排序)。
- `cli` lib:`resolve_constraints_opt` 加第 7 参数 `templates: Option<&str>`,在 `ts_inputs` 后写 lock 头部 `templates:` 行(单行化,`none` 占位)。新增 `read_lock_templates(path)` 只扫头部读回 `(name,version)`(`---` 后包体不误读)。
- `cli` lib:`maintain_from_lock` 在 propose 成功后读 lock 模板记录 → 写世代 templates.txt(旁路,不影响 propose/verify/激活)。
- `cli` main:`resolve --config` 分支用 `collect_templates` 取模板版本,经 `templates_record` 传入 resolve。
- 6 处 `resolve_constraints_opt` 调用点 + 委托函数补第 7 参数(无模板路径传 `None`)。

### 验收

- `template` 14 单测(+2:collect_templates 含 extends 链、环检测)。
- `cli/tests/template_integration.rs`(+3):lock 记录模板且 **closure_id 不受影响**、`read_lock_templates` 只扫头部不误读包体、record/read 世代模板往返。
- 既有 `ts_inputs_in_lock`/`repair_e2e`/`config_ts_consistency` 回归(补第 7 参数)全过。
- 全 workspace lib 单测全绿,0 warning。

### 边界与待办(诚实记录)

- **验收7 控制面已闭合**:走 maintain 主循环装系统时,世代自动记录所用模板。但 `resolve --config` 单独跑(不 maintain)只写进 lock,不建世代;世代记录依赖后续 maintain/install。
- **模板记录仍是纯审计**:不进 closure_id,verify 不校验其篡改(同 ts_inputs)。
- 模板记录走 lock 头部文本桥接,非结构化字段;`@` 作 name/version 分隔,模板名含 `@` 会歧义(模板名约定不含 `@`,未强制校验)。
- 仍未做(承前):重放消费 inputs/模板、相对 import 文件加载、复杂 semver range、恢复离线 vendor。



承第四十九/五十轮:TS 前端的 `useTemplate("minimal-desktop")` 此前只把模板名当包名 → 进未解析。本轮兑现 `docs/templates/01-template-model.md` 全部验收清单:模板 = 声明式蓝图,展开成约束集,接通 TS 前端。新建 `crates/template`,TS 与未来 TOML 前端共享。

### 关键决策(动手前 AskUserQuestion 锁定)

- **范围 = 完整模型一轮做完**(extends 继承/无环校验/optional 开关/layer_hint 校验/多模板合并)。
- **落点 = 新建 template crate**(前端无关共享层,非塞进 config-ts)。
- **格式 = 分节形态**:零依赖 `parse_toml_subset` 不支持 `[[array-of-tables]]`,故 `[capability.<id>]`/`[optional.<id>]`(跟随 foundation manifest 先例),文档已标注。

### 改动

- 新 crate `crates/template`(workspace 第 12 个成员)。`Template::parse`(分节 TOML)、`load_template(dir,name)`、`expand(dir, roots, ExpandOptions)`:
  - DFS 展开 `extends`(深度优先,先父后子),`visited` 防重复 + 路径栈检测继承环(验收2)。
  - 按展开顺序合并 capability,**后覆盖前**(子模板/后声明模板/override 胜出,验收3)。
  - optional `default` 开关 + `optional_switches` 覆盖(验收4)。
  - `layer_hint == foundation` → `ForbiddenLayer` 拒绝(验收6)。
  - constraint 串(`*`/`>=v`/`<=v`/`=v`/`>v`/`<v`/裸版本号)→ `Constraint`;模板只给约束不给 hash(验收5)。
  - BTreeMap 按 id 排序输出(确定性)。
- `config-ts`:`SynthOutcome` 加 `templates` 字段;`useTemplate(name)` 改为记进 templates(不再当包名);新增公开 `eval_to_outcome`(含 templates 的结构),`eval_to_constraints` 仍用于纯包场景。
- `cli`:`Layout::templates_dir()`;TS 分支用 `eval_to_outcome` → `aevum_template::expand` 展开模板(override/exclude 一并作用)→ 与直接 use 的约束合并(直接 use 覆盖模板同名)→ resolve。
- `cli`:`record_generation_templates`/`read_generation_templates` 旁路读写 `gen-NNN/templates.txt`(验收7,**不动 make_generation 签名**,跟随第46轮 keep-two.txt 先例)。
- 内置模板 `templates/minimal-desktop.toml`、`templates/dev-rust.toml`(extends minimal-desktop)。
- 文档:`01-template-model.md` 标注分节形态;`examples/aevum.config.ts` 注释 useTemplate 现已真展开。

### 验收

- `template` 12 单测:解析、layer_hint 拒 foundation、单模板展开、extends 子覆盖父、**继承环检测**、菱形继承无误报、override 胜出、exclude、多模板后胜、constraint 各形态、optional 开关、确定性。
- `config-ts` 15 单测(接入 templates 字段无回归)。
- `cli/tests/template_integration.rs` 3 测:dev-rust 展开含继承的 minimal-desktop 能力 + 默认 optional → 求出非空 lock;展开确定性;世代模板记录读写往返。
- `config_ts_consistency` 更新(useTemplate 语义变更:模板名 ≠ 包名),仍证 TOML 与 TS 前端 closure_id 一致。
- 端到端实跑:`aevum resolve --config examples/aevum.config.ts` → minimal-desktop 真展开成约束(coreutils/bash + 默认 optional),不再进未解析。
- 全 workspace lib 单测全绿,0 warning。

### 边界与待办(诚实记录)

- **分节形态偏离设计文档原 `[[capability]]`**:解析器限制下的等价折中(同 foundation),已文档留痕。
- **世代模板记录是旁路文件**(templates.txt),verify 不校验其篡改;且**未接入 activate 主循环**——本轮只提供 record/read 函数 + 测试,"世代构建时自动记录所用模板"是后续控制面工作。
- **constraint 解析是手写子集**:复杂 semver range(`^1.2`/`~1.2`/`>=1,<2`)不支持,遇到报错不静默。
- **layer_hint 仅做"禁 foundation"校验**,不做实际层分配(那是 Maintainer 职责)。
- **`[adds]` 派生语法糖**(README §3.2)未做,extends 已覆盖组合核心。
- 模板签名链(vendor 无 ed25519,同 foundation 先例)未做。



承第四十九轮:TS 前端的显式 inputs 只在求值时生效、未持久化。ADR-0004 明确要求"输入被记录进 lock,重放时输入也固定 → 仍可复现"。本轮兑现这个最小增量:inputs 写进 lock 头部审计区。

### 动手前的对抗式调研(workflow,4 路并行)

落地前用一个 understand workflow 把四件易踩的事查清并交叉验证:lock 写/读格式、新增段是否破坏 read_lock/verify、ADR 对 inputs 入 lock 的精确契约、inputs 是否该进 closure_id。结论一致确认了实现方向(下述),尤其排除了最大风险——`parse_lock_file` 头部解析宽松忽略未知行、verify 不对 lock 文本做逐字节/哈希校验,故新增段无害。

### 改动

- `resolve_constraints_opt` 末尾加可选参数 `inputs: Option<&str>`(与现有 `assist: Option<&AiAssist>` 同形,最小侵入)。
- lock 头部审计区(`ai_assist` 行之后、`---` 之前)新增 `ts_inputs:` 单行:有值写单行化后的输入(`\n\r\t` 替换为空格,防破坏按行解析),缺省写 `ts_inputs: none` 占位(对齐 ai_assist 的 None 分支风格)。
- 6 个调用点补末位实参:`main.rs` 的 TS 路径传 `inputs.as_deref()`,其余(intent/显式包名/maintain/委托函数/maintain_from_lock)传 `None`;`resolve_constraints` 委托传 `None`。

### 关键设计(经调研确认)

- **inputs 不进 closure_id**:closure_id 只是 resolved 包集 `(name,version,fingerprint)` 的内容摘要(`solver` build_lock)。inputs 是审计元数据——两份不同 inputs 若产相同约束就是同一系统、同一 closure_id。把 inputs 哈希进去会破坏去重/GC 可达性与"同输入两次求解 closure_id 一致"的确定性。
- **只写不验、只记不用**:本轮 inputs 是审计数据,verify 不校验其篡改(lock 文本无哈希校验),重放尚未消费它。

### 验收

- `cli/tests/ts_inputs_in_lock.rs` 4 测:inputs 单行化写进头部(`\n\t`→空格)、无 inputs 写 `none` 占位、**不同 inputs(及 None)产相同 closure_id**(证明纯审计不破坏确定性)、带 ts_inputs 段的 lock 经 `parse_lock_file` 回读干净(内容不混进包体)。
- 既有 `repair_e2e`/`config_ts_consistency` 等回归未受签名变更影响(调用点已同步)。
- 全 workspace 回归全绿:**157 测试**,0 失败 0 warning。

### 边界与待办(诚实记录)

- **重放闭环仍未闭**:inputs 已记进 lock,但"重放历史世代时重新注入记录的 inputs 求值"未做。**关键约束**:将来做重放消费时,缺失 inputs 必须显式报错,绝不能静默回退去读宿主环境变量(否则违反 ADR-0004 边界2 禁隐式环境读取)。
- **inputs 不入任何摘要**:不进 closure_id/intent_digest,故 verify 检测不到 ts_inputs 被篡改。属已知缺口(纯审计),非可复现闭环的破坏。
- **单行化有损**:`\n\r\t` 压成空格;inputs 来自 CLI 单参数通常本就单行,风险低。
- 仍未做(承第四十九轮):模板展开、resolved.toml 中间层、真抢占式求值超时、多文件相对 import 加载、SDK 签名分发、接入 maintain、恢复离线 vendor。



ADR-0004(TypeScript 作为可选第二前端)此前是 Accepted 但零实现。本轮兑现最小可验证链路:`aevum.config.ts` → boa 纯 Rust 沙箱求值 → 与 TOML 等价的约束 → 同一套确定性求解器 → 同一 lock。

### 关键决策(动手前用 AskUserQuestion 锁定)

- **求值引擎 = vendor boa(纯 Rust JS 引擎)**,不用外部 deno(用户明确否决运行期外部依赖,要可集成的)。boa 无 host 绑定 → FS/网络副作用天然不存在,只需主动禁时钟/随机。
- **范围 = 最小可验证链路**:type-strip → boa 沙箱 → 约束 → 复用 `resolve_constraints` → lock;最小 `@aevum/sdk`;一致性测试(同语义 TOML 与 TS 产相同 lock)。

### 改动

- 新 crate `config-ts`(workspace 第 11 个成员)。依赖 `boa_engine 0.21` + `aevum-solver`/`aevum-intent`。
- `eval_to_constraints(ts_source, inputs_json) -> Vec<Constraint>`:端到端入口。
  - **type-strip**:极简 TS→JS,删 import 行 / interface / type 别名 / 参数与变量类型注解。保守:括号内 `:` 仅在非嵌套 `{}`/`[]` 时当类型注解,避免误伤对象字面量 `{ version: "3.11" }`。
  - **boa 沙箱**:注入 `@aevum/sdk` 为 JS prelude(`defineSystem`/`useTemplate`/`sys.use`/`override`/`exclude`);prelude 把 `Math.random`/`Date.now` 覆盖为抛错(ADR-0004 禁随机/时钟)。
  - **import allowlist**(回应评审 H3):求值前静态扫描,仅放行 `@aevum/sdk` + 相对路径 `./ ../`;禁裸 npm 包名 / URL / 动态 `import()`。先剥行注释再扫描(注释里的 `import()` 不误触发)。
  - **显式 inputs**:JSON 对象注入为 `__aevum_inputs`,驱动条件/循环;同源同输入必产同约束(确定性)。
- `cli`:`aevum resolve` 加 `--config <ts>` 与 `--inputs <json>`;摊开沙箱产出的约束 → 确认 → 复用 `resolve_constraints_opt` 求解。
- `examples/aevum.config.ts`:可运行示例(条件启用、`for...of` 循环、override、exclude)。

### 验收

- `config-ts` 15 单测:type-strip 各场景、沙箱禁随机/时钟、import allowlist(拒 npm/动态、放行 sdk/相对)、注释不误触发、`for...of` 存活、use/override/exclude 端到端、显式 inputs 条件分支。
- `cli/tests/config_ts_consistency.rs` 3 测,**核心**:同语义 TOML 前端(coreutils+grep)与 TS 前端产出**逐字节相同 closure_id=clo-5499ad00ed71ad43(16 包)**——正面兑现 ADR-0004 红线:可复现只来自 lock,与前端无关。
- 示例配置端到端实跑:5 约束 → 真实 Debian 索引求解 → 50 包闭包 + closure_id。
- 全 workspace 回归全绿:**153 测试**,0 失败 0 warning。

### 边界与待办(诚实记录)

- **type-strip 是极简实现非完整 TS 编译器**:覆盖 ADR-0004 示例与常见意图配置(参数/变量注解、interface/type、import);泛型实例化 `foo<T>()`、装饰器、`enum`、多行 type 别名超范围,遇到交给 boa 报错(不静默猜改)。复杂配置可能需补强。
- **求值超时是软的**:boa 单线程不可抢占式取消,`DEFAULT_EVAL_TIMEOUT` 目前未真正中断死循环(图灵完备固有风险)。真抢占需 boa job queue/指令计数,标为待办。
- **相对 import 仅放行语法、未做真实文件加载**:多文件配置工程(`./helper`)本轮不解析加载,只允许其 import 语句通过 allowlist。真正的工程内多文件求值是后续工作。
- **`useTemplate(name)` 把模板名当普通包 use**:模板展开(模板名 → 一组包/覆盖)未实现(模板 crate 仍未建);示例里 `minimal-desktop` 因此进未解析。模板系统是独立后续项。
- **`@aevum/sdk` 是注入的 JS prelude,非真实分发的签名 npm 包**:ADR-0004 要求 SDK 走签名校验分发,本轮先内置实现 API 语义;签名分发是客户端打包阶段的事。
- **vendor 未重做**:`.cargo/config.toml` 当前指 ustc 在线镜像(非离线 `vendor/`),boa 136 传递依赖从在线拉取。恢复离线可复现(`cargo vendor` 重生成,43→~180 包)是独立基础设施收尾项,未做。
- TS 前端尚未接入 `aevum maintain` 主循环;`resolved.toml` 中间层与 inputs 入 lock 的记录格式(ADR-0004 要求 inputs 记进 lock 供重放)本轮未落地——目前 inputs 只在求值时生效,未持久化进 lock。



承第四十七轮:方案C 数据面(建视图、入世代、GC 安全)已齐。本轮接通运行期——用某 app 的私有视图作 `--library-path` 运行它,让"保留两份各跑各的"真正端到端成立。

### 改动

- `cli` 新增 `private_view_dir(layout, gen_id, app) -> Option<PathBuf>`:取 `gen-NNN/private-views/<app>`(存在才返回;含逃逸防护)。该目录可直接作 `run_isolated` 的 `--library-path`。
- `cli` 新增 `run_app_isolated(layout, gen_id, app, loader, bin, args)`:用该 app 私有视图作库搜索路径运行其二进制——同机两冲突 app 各自调用时各加载各版本依赖,互不可见(ai/02 §3.2 闭环)。无私有视图时返回 Err(调用方回退普通运行)。

### 验收

- `keep_two_isolation.rs` 世代级测试增强:`private_view_dir` 返回两 app 各自视图、视图里 `libfoo.so.1` 内容为各自版本(可作 library-path);无私有视图的 app/不存在的世代/逃逸名均返回 None。
- 全 workspace 回归全绿:**135 测试**,0 失败 0 warning。

### repair 全景(本阶段收尾)

| 阶梯 | 状态 |
|---|---|
| 冲突检测 | ✓ |
| A 放宽求共存版本 | ✓ 建议 + 自动应用 + 贯穿 resolve/maintain + 端到端 |
| B 升级父包求兼容 | ✓ 建议(确定性子情形)|
| C 保留两份 | ✓ 建议 + 隔离机制 + 世代集成 + GC 安全 + **运行期消费** |
| D 如实告知取舍 | ✓ 退化兜底 |

repair 阶梯 A→B→C→D **数据面与运行面均闭环**:从"检测冲突"到"保留两份各跑各的真正可运行",Aevum 标志性兜底能力成立。

### 边界与待办(诚实记录)

- `run_app_isolated` 需调用方提供 `loader` 与 `bin` 路径(最小可用);从世代自动定位 loader/主二进制是后续便利化。
- **尚未接入 maintain**:目前 `attach_keep_two_views`/`run_app_isolated` 是独立可调函数,把"方案C 被采纳 → 自动建私有视图入世代 → 提供运行入口"串进主循环是后续控制面工作。
- 未跑两个真实冲突 Debian 包各自 `run_app_isolated` 真实加载的全链路(需真冲突包;机制已由合成对象验证)。
- 方案B 降级子情形(功能损失评估,AI 域)仍未做。

---

## 2026-06-14(六)—— 第四十七轮:修 GC 可达性覆盖私有视图对象(闭合第四十六轮隐患)

承第四十六轮:私有视图对象不在 GC 可达性集,可能被误回收(已知隐患)。本轮闭合——把私有视图引用的对象纳入世代可达性。

### 改动

- `attach_keep_two_views` 写 `gen-NNN/private-objects.txt`(私有视图引用的全部 store 对象 id,去重有序)。
- `generation` `generation_object_ids` 改为**合并读** `lock.txt`(共享布局)+ `private-objects.txt`(私有视图),结果去重。`compute_garbage` 与 verify 完整性校验都经此函数,故两类对象都自动纳入可达性/校验。

### 验收

- `keep_two_isolation.rs` 世代级测试增强 GC 断言:私有视图对象(aaaa/bbbb)在 `generation_object_ids` 可达集、`compute_garbage` 把它们归 `kept` 不进 `garbage`、孤儿对象仍被回收。
- `generation` crate 旧 GC 单测不受影响(无 `private-objects.txt` 时合并逻辑跳过缺失文件)。
- 全 workspace 回归全绿:**135 测试**,0 失败 0 warning。

### 边界与待办(诚实记录)

- 私有视图对象现纳入 GC 可达性与 verify 完整性(经同一 `generation_object_ids`)——后者意味着有私有视图的世代,verify 也会校验其私有对象完整性,符合预期。
- 仍未做:`attach_keep_two_views` 接入 maintain、运行期消费私有视图(`--library-path`)、真冲突包全链路(承第四十六轮)。

---

## 2026-06-14(六)—— 第四十六轮:repair 方案C 世代级集成(旁路私有视图,"保留两份"随世代走)

承第四十五轮:隔离机制最小可验证。本轮把它接进世代——方案C 的私有依赖视图作为**旁路子树**挂进世代,让"保留两份"随世代走、可回滚,且不破坏现有世代/bootroot/GC。

### 范围决策(动手前用 AskUserQuestion 确认)

**旁路私有视图**:世代共享布局(`packages/`)不改,在世代下新增 `gen-NNN/private-views/<app>/` 旁路子树。避免改世代核心模型(那会动 make_generation/bootroot/全部世代测试)。

### 改动

- `generation` crate 加公开方法 `generation_dir(id) -> PathBuf`(暴露原私有 `gen_dir`),供上层在世代下挂旁路产物。
- `cli` 新增 `attach_keep_two_views(layout, gen_id, views)`:在 `gen-NNN/private-views/<app>/` 下调 `materialize_isolated_views` 建各 app 私有视图,写 `gen-NNN/keep-two.txt` 记录哪些 app 有私有视图(运行期/审计)。现有世代/bootroot/GC 只看 `packages/`,不受影响。

### 验收

- `keep_two_isolation.rs` +1:世代级集成端到端——造世代主体 + `attach_keep_two_views` 挂两 app 私有视图,断言 `private-views/<app>/` 各指向不同版本对象、各读各版本内容、`keep-two.txt` 记两 app、**世代主体 `packages/` 共享布局不受影响**。
- 全 workspace 回归全绿:**135 测试**(此前 134 + 1),0 失败 0 warning。

### repair 方案C 状态更新

| 子项 | 状态 |
|---|---|
| 保留两份决策建议 | ✓(四十四轮)|
| 运行时视图隔离机制 | ✓ 最小可验证(四十五轮)|
| 世代级集成(私有视图入世代 + 归属记录) | ✓ 旁路方式(本轮)|
| maintain 自动触发(方案C 采纳→自动建视图入世代) | ✗ 后续 |
| 真冲突包全链路运行 | ✗ 后续 |

### 边界与待办(诚实记录)

- 私有视图是**旁路子树**,与世代共享布局(`packages/`)并存但独立;运行期需用 `private-views/<app>/` 作 `--library-path` 才生效——这一步(运行期消费私有视图)尚未接入任何运行命令。
- `attach_keep_two_views` 是独立可调编排,**尚未接入 maintain**:把"方案C 被采纳 → 自动为冲突 app 建私有视图入世代"串进主循环是后续。
- GC 可达性目前只扫 `packages/` 的 `lock.txt`;私有视图引用的对象**未纳入 GC 可达性**——若私有视图对象不在 `packages/` 中,可能被误回收。**已知待办**(私有视图对象需加进世代可达性集)。

---

## 2026-06-14(六)—— 第四十五轮:repair 方案C 运行时视图隔离落地(最小可验证,"保留两份"真正可运行)

承第四十四轮:方案C 此前只产决策建议,运行时隔离列为"待定稿"。本轮落地其**最小可验证核心**——为两个冲突 app 各建私有依赖视图,同名依赖各指向不同版本 store 对象,证明 ai/02 §3.2 的"两份并存、互不可见"语义契约成立。

### 范围决策(动手前用 AskUserQuestion 确认)

**最小可验证隔离**:新增 `materialize_isolated_views`,不改世代模型。调研确认抓手已齐——`materialize_view`(按 rel_path symlink 回 store)+ `run_isolated`(`--library-path` per-二进制库视图)。世代级集成(per-app 私有 lib 子树)留作后续。

### 改动

- `cli` 新增 `materialize_isolated_views(views: &[(app, entries)], base_dest) -> Vec<视图目录>`:为每个冲突 app 在 `base_dest/<app>` 建独立视图(复用 `materialize_view`),视图里依赖 symlink 指向该 app 该用的那版 store 对象。app 视图名校验(拒 `/`、`\`、`..`,防视图逃逸)。
- 隔离运行配合 [`run_isolated`]:app 私有视图目录作 `--library-path`,ld 按 soname 命中的就是该 app 那版库。

### 验收

- 新增 `crates/cli/tests/keep_two_isolation.rs`(unix,离线确定性):
  - 两版本 `libfoo.so.1`(不同内容→不同 hash,store 天然并存)+ app-old/app-new 各自 entry → `materialize_isolated_views` → 断言两视图 symlink target 指向**不同 hash 对象**、且各自读到各自那版库内容(隔离成立)。
  - 视图名含 `..` 被拒(逃逸防护)。
- 全 workspace 回归全绿:**134 测试**(此前 132 + 2),0 失败 0 warning。

### repair 方案C 状态更新

| 子项 | 状态 |
|---|---|
| 保留两份决策建议(`suggest_keep_two`) | ✓(第四十四轮)|
| 运行时视图隔离机制 | ✓ 最小可验证(本轮:两 app 各见各版本)|
| 世代级集成(per-app 私有视图入世代 + lock 归属) | ✗ 后续 |
| 端到端"装两个真冲突包并各自运行" | ✗ 后续(需真冲突包 + 真库) |

### 边界与待办(诚实记录)

- 本轮验证的是**视图构造正确性**(symlink 指向正确版本对象 + 内容正确),用合成库对象;未跑"两个真实冲突 Debian 包各自 `run_isolated` 真实加载"的全链路(需真冲突包)。
- 隔离视图**尚未接入世代/maintain**:`materialize_isolated_views` 是独立可调函数,把它接进"方案C 被采纳后自动为冲突 app 建私有视图并入世代"是后续世代级集成。
- ai/02 §3.2 的语义契约("两份并存且互不可见")现有最小实现支撑;完整运行时隔离(命名空间/搜索路径在真实多包场景的健壮性)仍需更大验证。

---

## 2026-06-14(六)—— 第四十四轮:repair 方案C,保留两份建议(repair 阶梯 A→B→C→D 全闭环)

承第四十三轮:补 ai/02 §2/§3 **方案C**(保留两份)的决策建议。A/B 无解时,若冲突依赖能拆成两组、各组在 index 各有满足版本(版本不同)→ 建议两份并存,各闭包引各自 hash。补齐 repair 阶梯最后一格(确定性诊断部分)。

### 范围决策(动手前用 AskUserQuestion 确认)

**只做保留两份决策建议**,与 A/B/D 同模式。运行时视图隔离落地(两 app 各见各版本)不在本轮——但调研发现现成基础已在:`run_isolated`(PoC-2)的 `<loader> --library-path` 已是 per-二进制库视图,后续落地有抓手。

### 改动

- `solver` 新增 `suggest_keep_two(index, dep, constraint_sources)`:按"约束满足的最高版本"聚合来源,≥2 个不同版本各能满足一部分来源 → 产 `KeepTwoSuggestion{package, version_a/sources_a, version_b/sources_b}`(取最高两组)。
- `resolve` 冲突阶梯调整为 **A→B→C→D**:B 无解 → 试 C(保留两份);C 也分不出才落 D。
- `Diagnostics` 加 `keep_two_suggestions`;`cli` warn_conflicts 打印方案C(标需确认:占盘+各自安全更新)、lock 写 `# repair-C:`。

### 重要发现(诚实记录)

插入方案C 后,**方案D 在纯版本冲突下几乎不可达**:只要冲突两方各自单独有满足版本,C 就总能保留两份兜住——这正是 ai/02"保留两份是终极兜底"的体现。D 仅在退化情形(某约束的满足集为空,通常已是 unresolved 而非 conflict)触发,作为防御性兜底保留。第四十三轮的 D 测试场景(libc6 `>=2.36`/`<=2.34`)现正确地由 C 接管。

### 验收

- `solver` 单测 29 → **31**(原 2 个 D 测试重构为 4 个 C 测试:保留两份端到端、纯函数、同版本不建议、B 有解不升 C/D)。
- `repair_e2e.rs`:原方案D 端到端测试改为方案C(`# repair-C: libc6 保留两份 2.34 与 2.36`)。
- 全 workspace 回归全绿:**132 测试**,0 失败 0 warning。

### repair 阶梯最终状态

| 方案 | 状态 |
|---|---|
| 冲突检测 | ✓ |
| A 放宽求共存版本 | ✓ 建议+自动应用+贯穿+端到端 |
| B 升级父包求兼容 | ✓ 建议(确定性子情形)|
| C 保留两份 | ✓ 决策建议(运行时视图隔离落地待后续,基础 `run_isolated` 已在)|
| D 如实告知取舍 | ✓ 退化兜底(C 之后几乎不可达)|

**repair 阶梯 A→B→C→D 确定性诊断部分全闭环。**

### 边界与待办(诚实记录)

- 方案C **只产决策建议**,不落地运行时视图隔离(为两 app 各构造指向不同版本的 `--library-path`)——这是 Aevum 核心卖点的完整实现,工程量大,后续单独做。
- 方案C 的"两份"目前取满足来源的最高两组;>2 个不同版本需求时只建议两份(>2 份并存的语义未展开)。
- 方案B 降级子情形(功能损失评估,AI 域)仍未做。

---

## 2026-06-14(六)—— 第四十三轮:repair 方案D,隔离失败如实告知(repair 诊断闭环 A→B→D)

承第四十二轮:方案 A/B 已就绪。本轮补 ai/02 §2 **方案D**——A 无单一共存版本、B 无可升级父包时,确定性自动修复手段穷尽,**如实告知用户"需二选一取舍",绝不静默删除某一方**。repair 诊断阶梯由此闭环(A→B→D 全覆盖)。

### 改动

- `solver` `Diagnostics` 加 `unrepairable: Vec<UnrepairableConflict{package, constraints, sources}>`。
- `resolve` 冲突循环:方案A 无解 → 试方案B;**B 也无建议** → 推方案D(列出无法共存的依赖、互斥约束、来源父包,供用户判断取舍哪一方)。
- `cli`:`warn_conflicts` 打印方案D(`✗ 方案D: X 无法共存…需你二选一`);lock 诊断段写 `# repair-D: ...`。

### 验收

- `solver` 单测 27 → **29**(+2):A/B 都失败时升级方案D(顶层互斥约束、无父包可升)、B 有解时**不**触发 D。
- `repair_e2e.rs` +1:方案D 写入 lock(`# repair-D`)端到端。
- 全 workspace 回归全绿:**130 测试**(此前 127 + solver 2 + e2e 1),0 失败 0 warning。

### repair 阶梯完整状态(确定性部分收尾)

| 方案 | 状态 |
|---|---|
| 冲突检测 | ✓ |
| A 放宽求共存版本 | ✓ 建议 + 自动应用 + 贯穿 resolve/maintain + 端到端 |
| B 升级父包求兼容 | ✓ 建议(确定性子情形;降级属 AI 域,未做) |
| D 如实告知需取舍 | ✓ |
| C 保留两份 | ✗ 依赖运行时视图隔离(ai/02 §3.2,待代码阶段定稿) |

### 边界与待办(诚实记录)

- 方案 A/B/D 均为求解期诊断;A 可自动应用,B/D 只产建议/告知(B 的"升级父包"采纳前需上层校验父包自身约束,见第四十二轮)。
- 方案 C(保留两份)是唯一未覆盖的阶梯,它需要运行时视图隔离机制(同一机器两 app 各见不同版本),文档明确列为"待代码阶段定稿",非确定性求解器范畴。
- 方案 B 降级子情形(功能损失评估)属 AI 启发式,未做。

---

## 2026-06-14(六)—— 第四十二轮:repair 方案B,升级父包求兼容(确定性子情形,产建议)

承第四十一轮:方案A 完整收尾。本轮做 ai/02 §2 **方案B** 中确定性可算的部分——方案A 无解时,尝试把"提出过严约束的父包"升级到一个对该依赖约束更宽松的版本,使冲突依赖重获共存版本。

### 范围决策(动手前用 AskUserQuestion 确认)

**只做"升级父包"子情形,产建议**。方案B 的"降级一方"涉及功能损失评估(AI 启发式,非确定性),与"求解器纯确定性"边界冲突,不在本轮。与方案A 同模式:先建议,不自动应用。

### 改动

- `solver` 新增:
  - `extract_constraint_for(depends, dep_name)`:从 Depends 字段提取针对某依赖的版本约束(裸依赖/不含则 None)。
  - `suggest_upgrade_parent(index, dep_name, parent_constraints, other_constraints)`:逐个父包枚举候选版本(版本升序、取第一个可解),用「该版本对 dep 的新约束 + 其它父包约束 + other」跑 `find_satisfying_version`;得共存版本则产 `RepairSuggestionB{dependency, parent, upgrade_parent_to, dependency_version}`。
  - `resolve` 收集带来源的约束(`pkg_constraint_sources`);对方案A 无解的冲突包,拆「具名父包约束 / 其它约束」后调方案B,挂 `Diagnostics.repair_suggestions_b`。
- `cli`:`warn_conflicts` 打印方案B 建议;lock 诊断段写 `# repair-B: 升级 X 到 Y → dep 取 v`。

### 验收

- `solver` 单测 24 → **27**(+3):`extract_constraint_for` 解析、`suggest_upgrade_parent` 纯函数、resolve 端到端(app-x 钉低版致 openssl 冲突 A 无解 → 方案B 建议升级 app-x 到 1.2)。
- `repair_e2e.rs` +1:方案B 建议写入 lock(`# repair-B`)端到端。
- 全 workspace 回归全绿:**127 测试**(此前 123 + solver 3 + e2e 1),0 失败 0 warning。

### 边界与待办(诚实记录)

- **只产建议,不自动应用**(方案B 自动应用需重求解,且"升级父包"可能违反父包自身的其它约束——见下)。
- `suggest_upgrade_parent` 当前**不校验**建议的父包升级版本是否满足该父包自身的其它约束(如顶层把 app-x 钉 `<<1.1`,方案B 仍可能建议升到 1.2)。这是探索性建议(ai/02 说方案B 属 AI 评估),给上层"要解冲突需放宽父包到 X"的信息;采纳前需上层校验。**已知边界**。
- "降级一方"子情形未做(功能损失评估属 AI 域);方案 C(保留两份)仍依赖运行时视图隔离(待定稿)。

---

## 2026-06-14(六)—— 第四十一轮:repair 方案A 端到端测试(补第四十轮标注的缺口)

承第四十轮:`--repair` 贯穿了但只有 solver 层单测,缺端到端验证。本轮补上——在 CLI 层用真实数据(合成 index + 文本 lock)验证 repair 闭环。

### 改动

- 新增 `crates/cli/tests/repair_e2e.rs`(跨平台、离线、确定性):
  - 合成 index(libc6 三版本 2.34/2.35/2.36)写入临时 layout,构造互斥但有交集的约束(`>=2.35` 与 `<=2.35` → 交集 2.35)。
  - `repair=true`:断言 lock 无冲突、libc6 钉到共存版本 2.35、lock 文件含 `# applied-repair: libc6 钉到 2.35` + `conflicts: 0`。
  - `repair=false`:断言 lock 保留 `# conflict:` 诊断 + `# repair-A: ... 可放宽到 2.35` 建议、但**无** `# applied-repair`(未自动应用)。

### 验收

- 全 workspace 回归全绿:**123 测试**(此前 121 + repair_e2e 2),0 失败 0 warning。
- 这两个测试锁住了 repair 方案A 的完整契约:从"检测+建议"(repair=false)到"自动应用到可用 lock"(repair=true)的行为差异,且不依赖网络/unix。

### 边界与待办(诚实记录)

- 端到端覆盖的是 `resolve_constraints_opt` 层(求解→写 lock);更上层的 `aevum maintain --repair`(求解→propose→门禁)未单独端到端测,因其求解段与本测试同路径,且 propose 需真下载(由 `maintain_loop.rs` 覆盖主链路)。
- 方案 B(升降级)/C(保留两份)仍未实现。

---

## 2026-06-14(六)—— 第四十轮:repair 贯穿 maintain 主循环(`aevum maintain --repair`)

承第三十九轮:`--repair` 只在 `aevum resolve` 暴露。本轮把它贯穿 maintain 端到端主循环——求解阶段即可自动修复冲突,让主循环具备自愈能力。

### 改动

- `cli` `maintain(...)` 加 `repair: bool` 参数:求解从 `resolve`(裸 unconstrained)改为 `resolve_constraints_opt(..., repair)`(显式包名转约束后求解,支持方案A 自动应用)。
- `aevum maintain` 加 `--repair` 开关,贯穿两路:显式包名路径透传给 `maintain`;意图路径的 `resolve_constraints` 升级为 `resolve_constraints_opt(repair)`。
- repair 仍只发生在 lock **之前**(求解阶段),propose/verify/激活全程无 AI、无放宽(可复现只来自 lock,ADR-0003/0005 不变)。

### 验收

- 全 workspace 回归全绿:**121 测试**,0 失败 0 warning(本轮为参数贯穿,逻辑由第三十九轮的 solver 单测覆盖,无新增测试)。
- maintain 调用点(`maintain_loop.rs`)同步更新签名。

### 边界与待办(诚实记录)

- 本轮是参数贯穿,未新增端到端 repair 测试(需构造真实冲突的 index + 镜像,成本高);方案A 的修复逻辑已由 solver 层 `resolve_with_repair` 的 3 个单测充分覆盖。
- 方案 B(升降级)/C(保留两份)仍未实现;无单一共存版本的冲突仍只标记 unresolved。

---

## 2026-06-14(六)—— 第三十九轮:repair 方案A 自动应用(放宽 → 重求解 → 可用 lock)

承第三十八轮:方案A 只产建议。本轮让它**落地**——对有单一共存版本的冲突包,自动钉到该版本重求解,把"建议"变成可用 lock。ai/02 §2 方案A 的完整闭环。

### 改动

- `solver` 新增 `resolve_with_repair(index, template, base_overrides) -> RepairedResolution { resolution, applied, unresolved_conflicts }`:
  - 循环:`resolve` → 对有交集建议(`satisfying_version=Some`)且未被 Pin 的冲突包,加 `Override::Pin{Eq, v}` → 重 resolve;直到无冲突,或剩余冲突都无交集(转 B/C)。
  - **收敛性可证**:每轮至少把一个有交集冲突包钉死,该包约束统一为 `=v` 不再冲突 → 有交集冲突集严格单调减,最多「冲突包数」轮收敛(外加 +8 保护上限兜底)。
  - 复用现有 `Override::Pin` 机制(不新造求解路径);尊重调用方已有的 Pin(不覆盖)。
  - `AppliedRepair{package, pinned_version}` 记录应用了哪些放宽。
- `cli`:`resolve_constraints` 拆出 `resolve_constraints_opt(..., repair: bool)`(原函数委托它,repair=false,签名不变);`repair=true` 走 `resolve_with_repair`,把应用的放宽写进 lock 的 `# applied-repair:` 段。
- `aevum resolve` 加 `--repair` 开关(显式包名与意图两路都支持):默认只检测+建议,`--repair` 自动应用方案A。

### 验收

- `solver` 单测 21 → **24**(+3):冲突自动钉到共存版本后无冲突(`>=2.35` 与 `<=2.35` → 钉 2.35)、无交集时不应用且标记 unresolved(`>=2.36` 与 `<=2.34`)、无冲突时 no-op。
- 全 workspace 回归全绿:**121 测试**(此前 118 + solver 3),0 失败 0 warning。

### 边界与待办(诚实记录)

- 方案A 自动应用通过 `Pin(Eq, 交集版本)` 实现;**钉死后该包不再随其它约束浮动**——这正是"放宽到共存版本"的语义,但意味着后续若该版本本身引依赖冲突,需再一轮(已在收敛循环内处理)。
- `aevum maintain` 暂未接 `--repair`(其求解走 `resolve`);本轮只在 `aevum resolve --repair` 暴露。把 repair 贯穿 maintain 主循环是后续小步。
- 方案 B(升降级)/C(保留两份)仍未实现;无单一共存版本的冲突仍只标记 `unresolved_conflicts`。

---

## 2026-06-13(五)—— 第三十八轮:repair 方案A,放宽约束求单一共存版本(产建议,不改 closure)

承第三十七轮:冲突已可检测。本轮实现 ai/02 §2 **方案 A**——对冲突包,在 index 候选里找是否存在同时满足其全部约束的单一版本(放宽即可共存,最干净)。

### 范围决策(动手前用 AskUserQuestion 确认)

**只产修复建议,不改 closure/lock**。自动应用(重选版本+重新展开依赖+改写 closure)风险大(不同版本依赖不同、需重新求解、可能引入新冲突需迭代收敛),留作后续。本轮是确定性纯函数的建议生成。

### 改动

- `solver` 新增纯函数 `find_satisfying_version(index, name, &[(op,ver)]) -> Option<String>`:遍历该包候选版本,保留满足**每一条**约束者,取最高版本;无则 None。
- `resolve` 收集每个包的全部带版本约束(`pkg_constraints`,去重保确定性);求解结束后对每个冲突包算 `RepairSuggestion { package, constraints, satisfying_version }` 挂进 `Diagnostics.repair_suggestions`。
- `Diagnostics` 加 `repair_suggestions: Vec<RepairSuggestion>`。
- `cli`:`warn_conflicts` 在冲突后打印方案A 建议(可放宽到 X / 无单一版本需 B/C);`resolve_constraints` 把建议写进 lock 诊断段(`# repair-A: ...`)。

### 验收

- `solver` 单测 18 → **21**(+3):纯函数找到交集版本(`>=2.34` 与 `<=2.36` → 2.36)、纯函数无交集返回 None(`>=2.36` 与 `<<2.35`)、resolve 端到端冲突包带 suggestion(`<=2.34` 与 `>=2.36` → None + 收集 2 约束)。
- 全 workspace 回归全绿:**118 测试**(此前 115 + solver 3),0 失败 0 warning。

### 边界与待办(诚实记录)

- **只产建议,不自动应用**:建议不改 closure/lock,采纳与否由上层决定;真正"放宽后重求解闭包"是后续。
- 方案 A 失败(无单一共存版本)时本轮只标记"需 B/C",**方案 B(升降级)/C(保留两份)未实现**;C 还依赖运行时视图隔离(ai/02 §3.2,待代码阶段定稿)。
- 建议基于"被收集到的约束";若某约束来自未进入闭包的分支,可能不在集合内(与冲突检测同源,边界一致)。

---

## 2026-06-13(五)—— 第三十七轮:repair 前提,solver 版本冲突检测(从"静默吞"到"显式报")

承第三十六轮:maintain 主循环已通,但假设求解单一成功。本轮补 repair(ai/02)的**前提**——让 solver 检测版本冲突。此前 solver 对"同名包被互斥约束要求"是**静默取其一**(BTreeMap 后写覆盖先写),冲突被吞掉;ai/02 §1 说的"求解器报依赖冲突"根本不会发生。本轮把它变成显式报告。

### 范围决策(动手前用 AskUserQuestion 确认)

ai/02 完整阶梯(方案 A 放宽/B 升降级/C 保留两份/D 告知 + 运行时视图隔离)是大工程,且文档明说运行时隔离"待代码阶段定稿"。**本轮只做冲突检测**——repair 的最小可验证前提,不做修复决策。

### 改动

- `solver` `Diagnostics` 加 `conflicts: Vec<VersionConflict>`(package / chosen_version / required_op / required_ver / source)。
- `resolve` 主循环:工作队列元素携带「来源」(顶层意图为 `<template>`,依赖展开为父包名);处理某带 `op+ver` 的约束时,若该包已在 closure 且**已选版本不满足该约束** → 记一条冲突(互斥约束、无单一版本同时满足),保留已选版本不覆盖(确定性),继续求解。
- `cli`:`resolve_constraints` 把 conflicts 写进 lock 文件诊断段(`conflicts: N` + 每条 `# conflict: ...` 注释);`aevum resolve` 两路(显式包名/意图)都调 `warn_conflicts` 醒目提示。

### 验收

- `solver` 单测 15 → **18**(+3):检出互斥约束冲突(`>=2.36` 与 `<<2.35`)、兼容约束不误报(`>=2.34` 与 `>=2.36` → 2.36 满足)、冲突记录来源(`<template>`)。
- 全 workspace 回归全绿:**115 测试**(此前 112 + solver 3),0 失败 0 warning。

### 边界与待办(诚实记录)

- **只做检测,不做修复**:冲突被报告但 solver 仍产出 lock(保留先到约束选定的版本),下游 verify 闭合性可能因此放过——repair 的方案 A/B/C/D(放宽/升降级/保留两份/告知)是后续。
- 冲突检测覆盖"已选版本 vs 后到约束";因 closure 按包名排序处理,同名多约束的检测顺序确定但"谁先入 closure"由排序定——对"是否存在冲突"的判定充分,但冲突条目里的 chosen_version 取决于该顺序(已在注释说明)。
- 运行时视图隔离(保留两份的真正落地)仍属"待代码阶段定稿"(ai/02 §3.2)。

---

## 2026-06-13(五)—— 第三十六轮:C 主线,`aevum maintain --intent`(自然语言意图入口接上主循环)

承第三十五轮:maintain 主循环已通(显式包名)。本轮接上**自然语言意图入口**——`aevum maintain --intent "..."`,这才是 AI-native 定位的完整体现:从一句话到一个激活的世代。

### 改动

- **重构**:把 maintain 主循环的后半段(propose → verify 门禁 → 激活)抽成 `maintain_from_lock(layout, lock_name, ...)`,供两个入口共用。`maintain(packages)` 改为 `resolve` 写 lock 后调它(行为不变)。
- **cli/lib.rs**:`maintain_from_lock` 从已写好的 `locks/<name>.lock` 起跑——读回 lock → propose → 门禁。这样"AI 翻译意图"只发生在 lock 之前,**propose/verify/激活全程无 AI**(ADR-0003/0005:可复现只来自 lock)。
- **main.rs** `aevum maintain` 加 `--intent <text>` / `--mock` / `--yes`:
  - 意图路径:`IntentResolver` 翻译(DeepSeek 或离线 Mock)→ 摊开约束 → 交互确认(`--yes` 跳过,人在回路,ADR-0003 边界3)→ `resolve_constraints` 写 lock → `maintain_from_lock` 续跑。
  - 复用 `aevum resolve --intent` 的同款交互模式;包名/意图二选一。

### 验收

- `maintain_loop.rs` 加 intent 前段测试(不触网,确定性):自然语言"我要 python 环境" → Mock 翻译出 python3 约束(标记 AI 介入)→ `resolve_constraints` 求解出 35 包 lock、含 python3、文件落地。后半段(propose→门禁)由既有端到端测试覆盖。
- 全 workspace 回归全绿:**112 测试**(此前 111 + intent 1),0 失败 0 warning。

### 边界与待办(诚实记录)

- 真 DeepSeek 翻译需 `DEEPSEEK_API_KEY`(无 key 自动降级 Mock);测试只验 Mock 路径(确定性、离线)。
- repair / 保留两份(ai/02:求解冲突产多候选)未做;maintain 假设单一候选求解成功。
- 判据4①(CVE)、foundation 签名验签仍未实现(承前)。

---

## 2026-06-13(五)—— 第三十五轮:C 主线总成,`aevum maintain` 端到端主循环(并照出修复 put_file 真 bug)

承第三十一~三十四轮:verify 各段(resolve / propose / 门禁激活 / 判据)已就绪。本轮把它们串成 `aevum maintain` 端到端主循环——对应 `docs/ai/01-maintainer-loop.md` 主循环图(意图→求解→propose→verify→激活)。

### 改动

- **重构**:把 `install` 的"下载/解包/入库/补闭包/造世代"抽成 `propose_generation`(造候选世代**不激活**,世代状态机 propose 语义)。`install` 改为 `propose_generation` + `set_active`(行为不变,milestone1/7 回归验证)。
- **cli/lib.rs** 新增 `maintain(...)`:`resolve`(求解写 lock)→ `propose_generation`(造候选世代)→ `activate_verified`(verify 门禁激活)三段串成可复现链路。返回 `MaintainOutcome`(各阶段结果)。门禁拒绝不视为 Err(由 CLI 定退出码)。
- **main.rs** 新增 `aevum maintain <packages...> --gen --mirror [--lock] [--active-lock] [--foundation] [--confirm]`:分阶段打印①求解②propose③verify④激活;门禁拒绝时退出码 1(硬性失败)/2(需确认)。

### 抓到并修复的真 bug(诚实记录,verify 门禁的第一个战果)

接上 maintain 后端到端首跑即 verify 失败:`完整性: ld-linux-x86-64.so.2 期望 6bb6… 实得 6b10…`。**不是测试问题,是 `put_file` 潜伏已久的真 bug**:
- `put_file` 用 `read_meta(path)` 取 meta,对 symlink 源(loader/库常是解包目录里的软链)得到 `is_symlink=true`;但 `store.put` 用 `fs::write` 把内容**实体化**入库(对象永远是实体文件)。
- 于是 put 用 `is_symlink=true` 算 hash 存为目录名,get 重算时对象是实体文件(`is_symlink=false`)→ hash 必失配。
- install/bootroot 链路从不重算校验,故一直潜伏;**verify 门禁第一次真正校验 install 造的世代,立刻照出**。
- 修复:`put_file` 强制 `is_symlink: false`(与"内容实体化入库"的实际形态一致),只取源文件权限位。

> 这正印证 ADR-0003 的设计价值:独立机器校验能抓出提议方(install 流程)自己不会发现的不一致。

### 验收

- 新增 `crates/cli/tests/maintain_loop.rs`:真跑主循环(求解 hello 闭包 → propose → verify 门禁 → 激活),实测 **求解 4 包 → propose gen-50(345 store 对象)→ verify 通过 → active 切到 gen-50 + verified 标记**。网络/工具不可达则 skip。
- 全 workspace 回归全绿:**111 测试**(此前 110 + maintain_loop 1),0 失败 0 warning;milestone1/7(同用 put_file)未受 bug 修复影响。

### 边界与待办(诚实记录)

- maintain 目前走显式包名(无 AI 意图翻译);接 `--intent` 走 IntentResolver 是后续(resolve_intent 已就绪,串进 maintain 即可)。
- repair / 保留两份(ai/02:求解冲突产多候选)未做;maintain 假设求解成功单一候选。
- 判据4①(CVE)、foundation 签名验签仍未实现(承前)。

---

## 2026-06-13(五)—— 第三十四轮:C 主线,verify 判据3 落地(foundation manifest + Layer 约束)

承第三十三轮:verify 五判据此前只实现三条(完整性/闭合性/版本回退),判据3(Layer 约束)恒空、且 `foundation_provided` 传空导致 foundation 依赖被误报 unclosed。本轮实现判据3,消除该已知正确性局限。

### 方向决策(动手前用 AskUserQuestion 确认)

- **manifest 用 `[foundation.<name>]` 分节**(非文档 §2 的 `[[packages]]` 数组表头):零依赖 TOML 解析器 `parse_toml_subset` 不支持数组表头,改用语义等价的每包一分节,**不动解析器**、零新依赖。
- **本轮只做 verify 约束三部分**:required 在场 + 版本精确 + 并入闭合提供集;签名验签/启动期校验标为后续(vendor 离线无 ed25519 crate)。

### 改动

- `maintainer` crate 新增 `foundation.rs`(依赖 `aevum-service-compiler::parse_toml_subset`):
  - `FoundationManifest::parse(toml)` 解析 `[meta]` + `[foundation.<name>]` 段(version/required/upgrade_policy)。`required` 缺省视为 true(foundation 包默认必装),布尔写字符串 `"true"`/`"false"`(解析器只认字符串)。
  - `provided_names()` / `version_map()` 供 verify 取用。
- `verify` 签名增 `foundation: Option<&FoundationManifest>`(向后兼容,`None` 时行为同旧版),实现判据3:
  - ① **required 在场**:manifest 中 `required` 包不在 candidate 闭包 → `foundation_violations`(缺核心组件),硬性失败。
  - ② **版本精确**:在场的 foundation 包版本必须与 manifest 精确匹配,否则违规(防降级/篡改核心包)。
  - ③ **消除闭合误报**:manifest 全部包名自动并入 foundation 提供集,foundation 提供的依赖不再被报 unclosed。
- `cli`:`verify_generation` / `activate_verified` 增 `foundation_manifest_path: Option<&Path>` 参数;`aevum verify` / `aevum activate` 增 `--foundation <path>` 选项;门禁打印新增"层约束(判据3)"分项。

### 验收

- `maintainer` 单测 11 → **21**(+5 manifest 解析:包提取/required 默认/provided 映射/缺 version 报错/空 manifest;+5 判据3:required 在场版本匹配过、缺 required 失败、版本不符失败、非必装缺失不违规、**foundation 依赖不再误报 unclosed**)。
- 全 workspace 回归全绿:**110 测试**(此前 100 + maintainer 10),0 失败 0 warning。
- 文档同步:`layers/01-foundation.md` §2 加实现说明,标注实际 TOML 形态与 `[[packages]]` 的差异、以及签名/启动校验的待办。

### 边界与待办(诚实记录)

- 仍未实现:判据4①(CVE,需外部库)、foundation manifest **签名验签**与**启动期校验**(§2.1/§3,vendor 离线无 ed25519 crate)。
- 判据3 仅在调用方显式传 `--foundation` 时启用;不传则跳过(与旧版行为一致)。install/switch 路径未接 manifest。

---

## 2026-06-13(五)—— 第三十三轮:C 主线,verify 成为 activate 前置门禁(`aevum activate`,闭合 ADR-0003 安全模型)

承第三十二轮:verify 此前是可绕过的独立闸门(`set_active` 不校验)。本轮把它做成**激活前置门禁**——`aevum activate` 永不绕过 verify,真正闭合 ADR-0003"AI 无权自我放行"的安全模型。

### 方向决策(动手前用 AskUserQuestion 确认)

- **门禁放 CLI 编排层**(非 generation 状态机):新增 `aevum activate` 安全路径,`generation::set_active` 保持纯机械原子切换不变。改动小、不破坏现有 `switch`/`rollback`/`install`。完整六状态机(draft/candidate/verified/...)留待后续按需。
- **install 保持装完直接激活**:首装无 active 基线、verify 价值有限;门禁只作用于新的 `activate` 命令。

### 改动

- `cli/lib.rs` 新增 `activate_verified(layout, lock, gen, active_lock, confirm) -> ActivateOutcome`:
  - 先 `verify_generation`,按报告分流:**硬性失败**(`!passed`,完整性/闭合)→ 拒绝,`confirm` 也无法放行;**需确认**(版本回退)且未 `confirm` → 拒绝;通过(或 `confirm` 放行回退)→ `set_active` + 写 `verified` 审计标记。
  - `ActivateOutcome { report, activated, blocked_reason }`、`ActivateBlocked { HardFail, NeedsConfirm }`。
  - `confirm` 仅放行版本回退(人类知情拍板,ADR-0003 边界3),**永不**放行硬性失败。
  - 写 `gen-NNN/verified` 标记(记录判据结果 + 是否经人工确认),随世代走,可审计。
- `main.rs` 新增子命令 `aevum activate --gen <id> --lock <name> [--active-lock <name>] [--confirm]`:摊开判据、打印门禁结论;拒绝时 active 不动,退出码 `1`(硬性失败)/`2`(需确认未给);通过 `0` 并同步 boot default。与 `switch`(机械切换)并列,语义上是"安全激活"。

### 验收

- `verify_gate.rs` 新增 3 个门禁端到端测试(unix,真 store/世代/lock):干净候选激活成功(**active 指针真切 + verified 标记落地**)、缺依赖硬性失败(给 `confirm` 也拒绝、**active 不动**)、版本回退(无 `confirm` 拒绝→给 `confirm` 后激活)。
- 全 workspace 回归全绿:**100 测试**(此前 97 + 门禁 3),0 失败 0 warning。

### 边界与待办(诚实记录)

- 门禁是**显式安全路径**,非强制全局:`aevum switch`/`install` 仍可机械激活而不校验(按本轮方向决策保留)。若未来要求"任何激活都过门禁",需收口到统一 activate 或在 generation 层落状态机。
- 仍未实现判据3(foundation 层约束)、判据4①(CVE);`foundation_provided` 传空的已知局限不变(承第三十一/三十二轮)。
- `verified` 标记目前仅审计用,未被任何代码当作"必须存在才可后续操作"的不变量(无强制状态机)。

---

## 2026-06-13(五)—— 第三十二轮:C 主线,verify 接 CLI(`aevum verify`,真实世代/lock/index 端到端)

承第三十一轮:把纯库的 `verify` 接到真实数据上,成为可端到端调用的 `aevum verify` 子命令。打通 propose→**verify**→activate 状态机的中段闸门。

### 改动

- `generation` crate 暴露公开方法 `generation_object_ids(id) -> Vec<String>`(读世代 `lock.txt` 的 `<hash>-<name>` 列表);`compute_garbage` 重构为复用它(去内联读取,单一来源)。
- `cli` crate(依赖新增 `aevum-maintainer`):
  - `parse_lock_file(path) -> Lock`:把 `resolve_constraints` 写出的文本 lock 读回 `aevum_solver::Lock`(头部 + `---` + `name@version#fingerprint\tfilename`)。`package_count` 按实际行数重算,不信任文件头(防手改不一致)。
  - `verify_generation(layout, candidate_lock_name, candidate_gen, active_lock_name)`:编排三处数据——index(查 Depends 做闭合性)、candidate lock(版本语义)、候选世代 `lock.txt`(store 对象做完整性)、可选 active lock(版本回退比较)——调 `aevum_maintainer::verify` 产报告。
- `main.rs` 新增子命令 `aevum verify --gen <id> --lock <name> [--active-lock <name>]`:
  - 分判据打印(完整性/闭合性/版本回退各 ✓/✗/⚠;层约束与 CVE 明确标"本轮未实现")。
  - 结论行摊开 `passed` 与 `needs_user_confirm` 两个独立维度。
  - **退出码分流**(供脚本/CI):可自动激活 `0`、需人工确认(版本回退)`2`、硬性校验失败 `1`。

### 验收

- 新增端到端测试 `crates/cli/tests/verify_gate.rs`(unix,真 store/世代/lock/index):4 测试全过——干净候选可自动激活、缺依赖闭合性失败、版本回退强制确认、入库后篡改对象被完整性抓到。
- 全 workspace 回归全绿:**97 测试**(此前 93 + verify_gate 4),0 失败 0 warning。

### 边界与待办(诚实记录,承第三十一轮)

- 仍未实现:判据3(foundation layer 约束,缺 manifest 解析)、判据4①(CVE,缺外部库)。`foundation_provided` 当前传空——**已知局限**:若闭包真依赖 foundation 提供的包,会被误报 unclosed,待判据3 落地修正。
- verify 是独立闸门,**尚未自动串入** propose→verify→activate 全状态机(无"verify 通过才允许 set_active"的强制流程编排)。下一步可做完整 maintainer 循环骨架,把 verify 作为 activate 的前置门禁。

---

## 2026-06-13(五)—— 第三十一轮:C 主线,verify 安全闸门(独立机器判定,ADR-0003 H4 闭环切断)

C 主线(AI maintainer)第一步:实现 **verify** —— 拦在 candidate → verified 之间的独立机器判定,把"是否危险/是否需人确认"从 AI 自述改为 verify 阶段的确定性检查。对应 [`runtime/01-generation-lifecycle.md`](architecture/runtime/01-generation-lifecycle.md) §3 + §3.1、[`ai/01-maintainer-loop.md`](ai/01-maintainer-loop.md)。

### 为什么(回应评审 H4)

ADR-0003 边界1 限"AI 不直接选 hash",但 AI 仍能产出偏向旧版的约束(如 `==3.11.7`,含已知 CVE)间接达成危险结果——旧版有合法签名、过完整性/闭合/层校验,悄悄进 verified。更糟:`needs_user_confirm` 此前由 AI 自述,误标/恶意标"无需确认"即可绕过人类否决,形成"AI 既提议、又自评危险、还自己放行"的循环。verify 把危险判定独立于提议者。

### 改动

- 新增 crate `crates/maintainer`(纯确定性、无 AI/随机/时钟/网络,对齐 solver/store 可测风格;依赖 aevum-solver/aevum-store/thiserror)。
- `verify(candidate_lock, active_lock, index, store, object_ids, foundation_provided) -> VerifyReport`,实现 lifecycle §3 五条判据:
  1. **完整性**:逐个 store 对象(`<hash>-<name>`)调 `Store::get` 重算哈希校验。
  2. **闭合性**:candidate lock 内每包的 `Depends`/`Pre-Depends` 都能在 closure / foundation 提供集 / 虚包 provides 内满足(`a|b` alternatives 任一即可)。
  3. **Layer 约束**:本轮未实现(无 foundation manifest 解析),`foundation_violations` 恒空——**诚实标注待办**。
  4. **安全/版本回退**:① CVE 命中本轮未实现(需外部 CVE 库,待办);② candidate 某包版本 `deb_ver_cmp` 低于 active 同名包 → 标记版本回退,**强制人工确认**。
  5. `needs_user_confirm` 由 verify **机器独立判定**(版本回退命中即强制 `true`),不信任 AI 自述。
- `VerifyReport`:`passed`(硬性校验:完整性+闭合+层)与 `needs_user_confirm`(安全判据)是**两个独立维度**;`auto_activatable() = passed && !needs_user_confirm`。

### 关键数据流修正(动手前核实推翻了初版设计)

- lock 的 `fingerprint` = `sha256:<.deb 整包 SHA256>`(下载校验用);store 的 `hash` = `hash_blob` 对**解包后每文件**内容+mode 算的 **12-hex 前缀**。**二者语义不同**。
- 故判据1 的完整性输入是 candidate **世代**引用的 store 对象列表(`object_ids`),**不是** lock 的 fingerprint;判据2/4 用 lock 的版本语义。verify 同时接收两者,分别喂两类判据。

### 验收

- `cargo test -p aevum-maintainer`:11 单测全过——完整性(真对象通过/对象缺失/目录名不合法/篡改失配[unix-only])、闭合性(全依赖在场/缺依赖/foundation 提供/alternatives 任一)、版本回退(检出且强制确认/升级不误标/首装无 active 不比较)。
- 全 workspace 回归全绿:**93 测试**(此前 82 + maintainer 11),0 失败。

### 边界与待办(诚实记录)

- **判据3**(foundation layer 约束)未实现:需 foundation manifest 解析,`foundation_violations` 恒空。闭合性中 foundation 提供的依赖须由调用方 `foundation_provided` 显式喂入,否则会被误报 unclosed。
- **判据4①**(CVE 命中)未实现:需外部 CVE 库。
- verify 目前是纯库 + 单测,**未接 CLI**(`aevum verify <candidate-gen>` 留作下一步);也未串入完整 propose→verify→activate 状态机(C 后续)。

---

## 2026-06-13(五)—— 第三十轮:阶段4c,/etc 不可变基底 + overlayfs 可变层(QEMU 实证)

阶段4 第三步 4c:系统配置 `/etc` 做成"不可变世代基底 + overlayfs 可变上层"(对标 ostree/NixOS environment.etc)。

### 改动

- 新增 crate `crates/etc-builder`(纯函数、跨平台可测):`build_etc(toml)` → `EtcBase { files: Vec<EtcFile> }`。
  - `[system]` 段:`hostname`→`/etc/hostname`、`locale`→`/etc/locale.conf`(`LANG=`)、`timezone`→`/etc/timezone`。
  - `[files]` 段:`"相对路径" = "内容"` 生成任意 /etc 文件(支持嵌套如 `aevum/release`)。
  - 路径校验:拒绝绝对路径 / `..` 逃逸。输出按路径有序(内容寻址确定性)。
  - **复用 service-compiler 的极简 TOML 解析器**(`parse_toml_subset` 提为 pub),不引 toml crate(vendor 离线约束)。
- CLI 新增 `aevum etc build <toml...> --out <dir>`:多文件合并(后者覆盖同名),写 /etc 基底文件树。
- service-compiler 解析器增强:**支持 quoted key**(`"含/或.的键"` 剥引号 + 反转义),供 etc `[files]` 路径键用。
- `build-s6-boot.sh`(4c):/etc 走 overlay——基底放世代根 `/etc-lower`(含 s6 scandir + 声明生成的配置),init 挂 overlayfs(lower=`/etc-lower` 只读,upper=`/run/etc-rw` tmpfs 可变层)。
- 新增 `examples/etc/system.toml`(系统配置示例)。

### 抓到并修复的真问题(诚实记录,没糊弄)

- 首次 QEMU:`overlay 挂载失败,降级到 cp 铺基底`。**没当"差不多得了"**,加诊断查根因:`overlay 支持自检: 0` + `No such device` → 该内核(Debian linux-image 6.12)overlayfs 是**模块非内建**,而世代根没装/加载模块。
- 修复:从 Aevum 装的 linux-image 包取 `overlay.ko.xz` 解压进世代根 `/lib/modules`,init 里 `busybox insmod` 加载。
- 修复后复验:`overlay 支持自检: 1`、`/etc = overlay(真挂上,非降级)`。

### 验收(QEMU 端到端,真 overlay)

- `cargo test -p aevum-etc-builder`:6 单测全过(系统字段映射/任意文件/有序/拒路径逃逸/空配置/组合)。
- QEMU:`/etc = overlay(lower=/etc-lower + upper)`、`/etc/hostname = aevum-demo`(来自声明)、写 `/etc/local-test` 成功**且基底 `/etc-lower/local-test` 不存在**(证明本地写入落 upper、不污染只读基底)。s6 服务仍 up。
- **完整链路**:TOML 系统配置 → `aevum etc build` → /etc 基底(随世代走)→ init overlay 挂载 → 运行时不可变基底 + 可变上层。
- 全 workspace 回归全绿(见测试数)。

### 边界与待办

- upper 层此处用 tmpfs 演示(重启即失);真部署应用持久卷,且按 runtime/04 三类状态分别处理回退。
- overlay 模块靠脚本从 linux-image 包取 + init insmod;未来应由引擎/Foundation 统一管内核模块(min-toolset/Foundation 范畴)。
- /etc 基底随世代回退**机制成立但未单独做世代回退实测**(4d 的事:状态随世代回退)。
- 用户管理/网络/fstab 等具体 /etc 配置项是长尾,本轮只做了 hostname/locale/timezone + 任意文件通道。

---

阶段4 第二步 4b:把 4a 手写的 demo/run 泛化成"从纯数据 TOML 声明编译"。打通"声明式→确定性产物"范式到服务层。

### 路线决策(4b 起步)

- **选 s6 原生 scandir 机制**(非 s6-rc):Debian trixie 无 s6-rc 包(无编译期依赖图工具)。先用 scandir(每服务一目录 + 可执行 run,s6-svscan 监督),`deps`(after/needs)本阶段记录为元数据(写进 run 注释 + dependencies 文件)。s6-rc 依赖图编排留作后续增强(需先解决 s6-rc 来源)。低风险、延续 4a 已验证机制。

### 改动

- 新增 crate `crates/service-compiler`(纯函数、跨平台可测,对齐 bootloader.rs 模式):
  - `Service::parse(toml)`:解析服务声明(name/type/run.argv/run.user/run.env/deps.after/deps.needs)。
  - `compile_service()` → `CompiledService { run, type_file, dependencies }`:渲染 s6 scandir 服务文件集。
  - run 脚本:`#!/bin/busybox sh` + LD_LIBRARY_PATH(4a 机制)+ env + `exec` argv;指定 user 时用 `s6-applyuidgid` 降权。argv **逐项 shell 转义**(`sh_quote`,单引号法),不让声明者写 shell(配置即数据)。
  - **内置极简 TOML 子集解析器**(零依赖:项目 vendor 离线、无 toml crate):支持 `[section]`、`key="字符串"`(含 `\"`/`\\`/`\n`/`\t` 转义)、`key=["数组"]`、`key={内联表}`、`#` 注释(字符串内 # 不截断)。非通用 TOML,刻意限范围保持可控。
- CLI 新增 `aevum service compile <toml...> --scandir <dir> [--lib-path]`:编译声明进 scandir(写 run+chmod 0755、type、dependencies)。
- `build-s6-boot.sh`:demo 服务从手写 here-doc 改为 `examples/services/demo.toml` → `aevum service compile` 生成(引擎驱动)。
- 新增 `examples/services/demo.toml`(声明式服务示例)。

### 验收(QEMU 端到端)

- `cargo test -p aevum-service-compiler`:12 单测全过(解析/类型默认/拒空 argv/拒缺 name/拒坏类型/render_run/降权/编译产物/shell 转义/注释/# 不截断/转义引号)。
- `aevum service compile demo.toml`:产出正确 run(argv 转义、保留 shell 片段里的 `"` 和 `#`)。
- QEMU:`[demo-svc] 由 TOML 声明编译,tick=1..` 持续打印,`s6-svstat` 确认 `up (pid 141)`。**完整链路:TOML 声明 → service compile → scandir → s6 监督拉起 → 服务真跑**。
- 全 workspace 回归全绿(见下轮记录的测试数)。

### 开发中暴露并修复的解析器真缺陷(诚实记录)

- 初版极简解析器把**字符串内的 `#` 当注释截断**、且不处理 `\"` 转义 → demo.toml(argv 含 `echo "..."` shell 片段)解析失败。这是解析器真 bug(非过度简化借口)。已修:`strip_comment`/`split_top_commas` 跟踪引号状态并跳过 `\"`;`parse_string` 处理转义。加了对应单测。

### 边界与待办

- s6 原生 scandir 无显式依赖图:`deps` 目前是元数据,服务启动顺序未强制编排。**真依赖图(s6-rc)是下一个增强**,前置是解决 s6-rc 来源(上游 musl 静态构建 vs 放弃 s6-rc)。
- TOML 子集解析器非通用(不支持多行数组/嵌套表头/数字类型);服务声明用不到,按需扩展。
- /etc 系统配置(4c)、服务状态随世代回退(4d)仍待办。QEMU 非物理机;s6 动态链接(静态化待办)。

---

第27轮 4a 暴露的引擎缺口,本轮定位根因并修复。最初猜是"补闭包没建 soname 链",深查后发现**更精确的根因在 export_bootroot**。

### 根因(精确定位,与初猜不同)

- Debian 库包内本就有 soname 软链 `libskarnet.so.2.14 → libskarnet.so.2.14.3.0`。
- **store 层是对的**:`put_symlink` 正确保留了该软链对象(PoC-5 铁律,存 target 字节、取出重建)。
- **bug 在 `export_bootroot`**:重建 bootroot 布局时用 `src.is_file()` + `canonicalize` + `fs::copy` 处理每个 store 对象 —— 软链类对象 `is_file()` 判否(其相对 target 在该 store 对象目录内不存在)被 **continue 跳过**;即便不跳过,`canonicalize` 也会解引用成实体。结果:bootroot 的 multiarch 目录**只剩实体、丢了 soname 链** → loader 按 NEEDED(`libskarnet.so.2.14`)找不到 → s6-svscan 首引导 `cannot open shared object` → kernel panic。
- 这违反了 CLAUDE.md 明列的 PoC-5 铁律:**符号链接保留不解引用**。store 守住了,export_bootroot 没守住。

### 修复(crates/cli/src/lib.rs export_bootroot)

- 遍历 store 对象改用 `symlink_metadata` 判类型:
  - 软链对象 → 在 bootroot 用 `os::unix::fs::symlink` **重建同样的软链**(保留 soname→实体 关系,不解引用)。
  - 实体文件 → 复制内容 + 恢复权限位(PoC-6)。
- 不再用 `canonicalize`(它解引用软链)。

### 验收

- export-bootroot gen-60:multiarch 目录 soname 链全部保留为软链(`libskarnet.so.2.14`/`libs6.so.2.13`/`libexecline.so.2.9` → 各自实体)。
- **去掉 4a 脚本 workaround**(`build-s6-boot.sh` 不再手补 soname 链),纯引擎修复后重造镜像 → QEMU 引导:s6-svscan 作 PID1、demo 服务 `up (pid 140)`、tick 持续涨、无库错误。
- 新增回归测试 `crates/cli/tests/bootroot_soname.rs`(纯本地确定性,不依赖网络):构造含 soname 软链的 store + 世代,断言 export_bootroot 后 bootroot 里软链仍是软链且指向实体。
- 全 workspace 回归全绿(63 测试 / 14 目标,含 milestone 端到端 + 新回归测试)。

### 意义

- 这是个影响面超出 s6 的真 bug:**任何带 soname 软链的多库依赖包**(几乎所有非平凡 Debian 库)经 export_bootroot 都会丢链。hello 只依赖 libc(loader 名精确匹配)侥幸未暴露,s6 多库一来即现形——延续"复杂包让隐藏缺陷现形"的项目模式(PoC-5/第22轮同款)。
- 4a 的 s6 接管 PID1 现在是**纯引擎驱动**(无脚本侧 workaround)。

---

阶段4 设计草案(第26轮)落地第一步 4a:把 busybox-as-PID1 换成 s6 监督树。路线甲(用户确认):s6 来自 Debian 包,动态链接,用世代自带 libc+loader 跑;静态化(musl)留作待办。

### 改动

- `aevum install s6 execline --only s6,execline,libs6-2.13,libexecline2.9,libskarnet2.14t64 --generation 60`:引擎求解闭包(8 包 0 未解析)→ 下载校验解包入 store → gen-60(140 store 对象)。**复用里程碑5-7 已验证的真 Debian 流程,引擎零改动**(对齐 ADR-0006)。
- 新增 `scripts/build-s6-boot.sh`(阶段4a):引擎 export-bootroot 产 s6 世代根 → 组 scandir + 一个 demo longrun 服务(run 脚本每秒打印)→ 世代 `/sbin/init` 改为 `exec s6-svscan /etc/s6/scandir`(s6 成 switch_root 后的 PID1)→ FAT+syslinux 引导。

### QEMU 验证(成功)

```
=== Aevum 阶段4a:s6 接管 PID1 (gen-60) ===
Aevum generation root: gen-60
[demo-svc] s6 监督下运行,tick=1..32 (pid 143)   # 持续每秒打印
[verify] s6-svstat demo 服务状态:
up (pid 143 pgid 143) 5 seconds                  # s6 监督树自报状态
```

- **s6-svscan 作为 PID1 起来**(无库错误);**demo 服务被 s6-supervise 监督拉起**;`s6-svstat` 确认 `up`(真监督,非裸 exec);PID 稳定、监督树健康(QEMU 持续运行,非 panic)。

### 抓到的真引擎缺口(诚实记录,已记待办,未蒙混)

- **install/补闭包未按 ELF SONAME 建库软链**:Debian 库实体是 `libskarnet.so.2.14.3.0`,但二进制 NEEDED 的是 SONAME `libskarnet.so.2.14`(正常靠 ldconfig 生成链)。Aevum 入 store 只存实体、没建 soname 链 → loader 找不到 → **首次引导 s6-svscan 因 `libskarnet.so.2.14: cannot open shared object file` 崩、kernel panic**。
- hello(里程碑1-8)只依赖 libc(loader 名字精确匹配)未暴露此缺口;s6 多库依赖一来现形——与 PoC-5"复杂包现形"同模式。
- **4a 临时处置**:`build-s6-boot.sh` 按每个库的 ELF SONAME 补软链(脚本侧 workaround),验证 s6 可跑。
- **根因修复(下一步待办)**:让引擎补闭包(`ingest_closure`/`build_with`)读库 SONAME,在世代/bootroot 建 `soname→实体` 软链(类似 ldconfig 职责)。修完移除脚本 workaround。

### 边界与待办

- s6 来自 Debian 包(动态链接),静态化(musl,满足 Foundation min-toolset)待办。
- **Debian trixie 无 s6-rc 包**(只有 s6 监督工具,无依赖式服务管理器 s6-rc)。4a 只用 svscan 监督树不受阻;4b(服务编译器,需 s6-rc 的编译期依赖图)前须先解决 s6-rc 来源(上游静态构建或改用 s6 原生 scandir 机制)。已记入设计文档待办。
- 引擎 SONAME 建链缺口(见上)是 4b 前应优先修的真 bug。
- QEMU 非物理机;demo 服务是验证占位,非来自 TOML 声明(TOML→s6-rc 是 4b)。

---

阶段3 收尾后启动阶段4。按项目方法论"先设计、再实证",本轮**只产设计文档,不写实现**。落地 ADR-0006 第109行预留的 `docs/architecture/bootable/` 子目录。init 路线经调研 + 用户确认选定 **s6/s6-rc**。

### 新增文档

- `docs/architecture/bootable/README.md`:可引导各阶段索引 + 状态总览(阶段0-3 已完成、阶段4 设计中)。补登记阶段1-3(实现期未单独成文,凭据指向 CHANGELOG + 代码)。
- `docs/architecture/bootable/04-init-services-config.md`:阶段4 设计草案。

### 阶段4 设计要点

- **init 路线选 s6/s6-rc**(首选)。理由:PID1 极简、可静态链接(满足 Foundation min-toolset"最后防线")、**s6-rc 编译期依赖图与 Aevum"声明式→确定性产物"范式同构**、服务定义是静态文件非图灵完备脚本(对齐 ADR-0002)。次选 dinit。**排除 systemd**:动态依赖庞大、耦合、自成 DSL、抢配置权——与原子化世代哲学冲突(诚实标注代价:放弃 systemd unit 生态,需自维护服务声明)。
- **服务模型**:纯数据 TOML 声明(name/type/run.argv/deps/state/health)→ Aevum 服务编译器 → s6-rc source → `s6-rc-compile` → 不可变 db → 随世代走。`run` 用 argv 数组 + execline,不让用户写 shell(保持配置即数据)。
- **服务状态随世代回退**:TOML `[state]` 挂接 runtime/04 已定的状态快照契约。
- **/etc 管理**:不可变世代基底(TOML 配置 → 生成 /etc 内容寻址入 store)+ overlayfs 可变上层。对标 ostree 三路合并 / NixOS environment.etc。回退时基底随世代走、可变层按 runtime/04 三类处理。
- **架构落点**:引擎零改动(ADR-0006),只加外壳 `crates/service-compiler` + `crates/etc-builder` + 调系统工具(s6-rc-compile/mount,不引 Rust 依赖)。
- **子阶段拆分**:4a(s6 接管 PID1,最小)→ 4b(服务编译器)→ 4c(/etc overlay)→ 4d(状态随世代回退)→ 4e+(网络/用户/locale 长尾)。每步独立 QEMU 可验证。

### 边界与诚实声明(写进设计文档 §7)

- QEMU 非物理机;放弃 systemd 生态是真实工程负担;s6 静态链接与同源约束(里程碑4)交互待实测(可能需 musl);overlayfs+世代回退事务性是实现期硬课题(runtime/04 已声明);健康探测仅列接口。

### 下一步

设计待评审。通过后从 **4a(s6 接管 PID1)** 起步,延续阶段1-3 增量风格。

---

第24轮的诚实边界点了名:菜单渲染还在脚本 here-doc 里,`switch`/`rollback` 只改世代 `active` symlink、不动开机菜单 DEFAULT——所以"回滚"靠重造镜像模拟。本轮收掉它:把菜单渲染和 DEFAULT 改写提进引擎,`rollback` 成为一条真命令。

### 改动

- 新增 `crates/generation/src/bootloader.rs`(纯字符串 + IO,不引依赖、跨平台可测):
  - `BootMenu { default, kernel, timeout, entries, append }` + `render()`:渲染 syslinux.cfg,等价原脚本 here-doc 但由引擎产出。
  - `set_default(cfg, gen)`:只改 DEFAULT 行 + 刷新各 `MENU LABEL` 的 `(active/default)` 标记,原子写(临时文件 + rename),**不重渲整张菜单**——语义对齐世代 active 指针回指(不重建)。校验目标 `LABEL gen<id>` 存在,否则拒绝(不让 DEFAULT 指向不存在世代)。
  - `parse_default()` / `find_config()`:解析当前默认世代 / 探测 syslinux.cfg|extlinux.conf。
- CLI 新增 `aevum boot-menu --gens 50,51 [--kernel --timeout --append --out]`:引擎渲染菜单,第一个世代作 DEFAULT。
- `aevum switch`/`rollback`:切完世代 active 指针后,探测到 bootloader 菜单配置(`boot3-build/stage/syslinux.cfg`)就**同步改 DEFAULT**,并提示如何同步进 FAT 镜像。找不到配置(普通包管理场景)静默跳过。
- `build-bootimage.sh` 第3段:菜单从 here-doc 改调 `aevum boot-menu` 渲染——引擎驱动。
- `Layout` 加 `boot_dir()` / `boot_menu_cfg()`。

### 验收(引擎渲染字节对齐 + rollback 真命令端到端)

- 引擎渲染的 syslinux.cfg 与原脚本 here-doc 逐行对齐(DEFAULT、两个 LABEL、active 标记位置)。
- `aevum rollback 51` 一条命令:`[rollback] active → gen-51` + `[boot] 菜单 DEFAULT → gen-51`,DEFAULT 与 active 标记同步移到 51。
- 完整闭环 QEMU 实测:`rollback 51`(命令改 cfg)→ `mcopy` 同步进镜像(不重造)→ QEMU 开机内核加载 `initrd-51.gz` → 进 gen-51(`Aevum generation root: gen-51`)。
- `switch 50` 对称改回,复位干净。
- 全 workspace 回归全绿(62 测试 / 13 目标,generation 新增 4 个 bootloader 单测)。

### 与上轮边界的对比(这轮消掉的)

- 上轮:回滚靠"换 DEFAULT 重造镜像"模拟,菜单渲染在脚本里。
- 本轮:回滚是 `aevum rollback` 真命令,引擎改 cfg DEFAULT;菜单渲染在引擎 `BootMenu::render()`。

### 诚实标注的边界(仍在)

- QEMU 非物理机、上游共享内核、busybox 当 init。
- 引擎改的是 cfg **配置源**(`stage/syslinux.cfg`),要生效仍需把它同步进 FAT 镜像(一条 `mcopy -D o`,命令已提示);引擎不直接操作 FAT 镜像(mtools 是脚本/平台职责,不进核心 crate)。这是合理分层,非缺陷。
- 菜单交互式选择(↑↓选非默认项)仍未在串口文件模式实测(验的是 DEFAULT 路径,两方向覆盖)。
- 真 init 系统 / 服务 / 系统配置(阶段4+)仍是数月级长尾待办。

---

ADR-0006 阶段3。前几轮证了"从单个 Aevum 世代引导";本轮加 bootloader 层:一块磁盘上装多个可引导世代,开机出菜单选世代,改 active 指针即回滚。

### 改动

- 新增 `scripts/build-bootimage.sh`:为传入的每个世代各造一份 initramfs(引擎 `export-bootroot` 产物 + busybox init + switch_root),渲染 syslinux/extlinux 多世代菜单(`syslinux.cfg`),造 FAT32 镜像、装 syslinux 引导扇区、mtools 塞文件。**全程普通用户**(免 loop 挂载;`export-bootroot` 需 cargo,root 无工具链)。
- 菜单 `DEFAULT` = 传入的第一个世代(= active 指针)。每世代一个 `LABEL`,各自 `INITRD /initrd-<gen>.gz`,共享内核 `/vmlinuz`。

### 抓到并修复的真实 bug(2 个,都是 mtools/syslinux 交互坑)

- `mcopy` 撞名卡死:`syslinux --install` 会自行往 FAT 根写一份 `ldlinux.c32`,stage 里再 cp 一份 → mcopy 复制时弹交互覆盖提示、后台任务挂起。**修**:stage 不再 cp `ldlinux.c32`(交给 syslinux 装),只放 `menu.c32`+`libutil.c32`;mcopy 加 `-D o` 非交互覆盖兜底。

### 验收(QEMU 双向实测)

- **默认引导(gen-50 作 DEFAULT)**:syslinux 菜单出现(MENU TITLE "Aevum - 选择世代"),倒计时自动引导 gen-50 → `Aevum generation root: gen-50` → 世代自带 libc 跑 `Hello, world!` → `[gen-50] 这是从 bootloader 菜单选中的世代`。
- **回滚(gen-51 作 DEFAULT 重造,模拟 `rollback` 切 active 指针)**:引导进的是 gen-51 → `Aevum generation root: gen-51`、`[gen-51] ...`;gen-51 是 busybox 世代、无 hello(gen-50 才有),证明确实进了不同世代而非同一个。
- 回滚语义闭环:改 active 指针(DEFAULT)→ 重渲菜单 → 开机进不同世代,无需重建世代内容(世代是不可变内容寻址对象,指针重指即回滚,对齐 PoC-7)。
- 全 workspace 回归全绿(58 测试通过 / 13 目标,纯脚本改动无影响)。

### 诚实标注的边界(仍在)

- QEMU 非物理机、上游共享内核、busybox 当 init。
- 回滚目前靠"换 DEFAULT 重造镜像"模拟,**尚未接 `aevum rollback` 命令直接改菜单**(菜单渲染逻辑在脚本里,未进引擎)。真实形态应是引擎管理 bootloader 配置、`switch`/`rollback` 直接重写 `syslinux.cfg` 的 DEFAULT。
- 菜单交互式选择(↑↓ + 回车选非默认项)未在串口文件模式下实测(`-serial file:` 不便交互);验的是 DEFAULT 路径,两个方向都覆盖到。
- 真 init 系统 / 服务 / 系统配置(阶段4+)仍是数月级长尾待办。

---

补真那轮(二十二)留了个明确待办:`install` 只装包文件、不补运行闭包,所以 bootroot 的 libc 要**旁路** `export-rootfs` 补、不是世代自带。本轮收掉它:让 `install` 把运行闭包(libc+loader)也补进世代,世代真正自包含。

### 改动

- `install` 入库包文件后,对每个包**扫 ELF 补运行闭包**(`build_with` 内部扫全包):解出的库 → store + 世代(rel_path `usr/lib/<soname>`),loader → 世代(`usr/lib64/<loader>`)。纯库/数据包(无 usr/bin 可执行)跳过补闭包。
- 结果:`install hello` 的世代从 49 对象增至 52,自带 `usr/lib/libc.so.6` + `usr/lib/ld-linux-x86-64.so.2`。

### 验收(QEMU 全裸,世代完全自包含、无旁路)

```
[gen-init] 根标志: Aevum generation root: gen-50
[gen-init] / 布局(全来自世代): AEVUM_GENERATION_ROOT bin dev lib lib64 ...
[gen-init] /lib 内容(世代自带的 libc+loader): ld-linux-x86-64.so.2 libc.so.6
[gen-init] hello 用【世代自带】libc 跑(非旁路补): Hello, world!
[gen-init] 世代自包含 = 系统根。install→世代→引导 全链由引擎驱动。
```

- bootroot 全部内容(含 libc+loader)来自 `aevum export-bootroot 50`,**无任何旁路 export-rootfs**。
- 脚本 `build-bootroot-initramfs.sh` 简化:只调 export-bootroot + 补 busybox(平台 init 工具)+ 组 initramfs。
- `install → 世代(自带闭包) → export-bootroot → QEMU 引导` 全链由 Aevum 引擎驱动。

### 诚实标注的边界(仍在)

- 仍 QEMU、上游内核、busybox init。
- busybox 仍由脚本补(它是平台兜底工具,非 hello 包的依赖;未来应作 Foundation min-toolset 的一部分由引擎管)。
- 补闭包用宿主 libc(跨发行版,里程碑4 同源边界仍适用);多包完整环境、maintainer scripts、bootloader 多世代菜单(阶段3)、真 init 系统(阶段4+)仍是待办。

---

## 2026-06-11(日)—— 第二十二轮:补真,让 Aevum 引擎(而非脚本)驱动引导内容

阶段1/2 的 bootroot 是 shell 脚本手 cp 拼的,**Aevum 引擎没参与**——证明的是"Linux 能从目录引导"(常识),而非"Aevum 世代机制驱动引导"(价值)。本轮补这个被绕过的缺口,并因此挖出并修复世代模型的一个真实架构 bug。

### 挖出的真实架构 bug:世代丢了布局

`install` 用 `ingest_dir` 把包每个文件内容寻址入库,转 `PackageRef` 时**只留了 file_name、丢了 rel_path**(`usr/bin/hello` 退化成 `hello`)。世代 `packages/` 因此是一堆扁平散落文件,根本不知道"什么是包、文件该放哪"。`export-bootroot` 撞上它 → 只能产 2 个文件。脚本手拼时直接 cp 掩盖了这个缺陷,引擎驱动一上就现形。

### 修复:rel_path 贯穿世代模型

- `PackageRef` 加 `rel_path: Option<PathBuf>`。
- `make_generation` 按 rel_path 建**层级 symlink**(`packages/usr/bin/hello → store对象`),世代目录忠实记录布局(也解决多包同名文件互相覆盖)。
- `generation_refs` 改递归遍历,返回 `(rel_path, store_dir)`。
- `install`/`ingest_closure`/`compose_generation` 全程传真实 rel_path。
- `export_bootroot` 直接按 rel_path 重建布局(不再绕 unpacked 猜包名)。

### 验收(QEMU 全裸,引擎驱动)

```
[initramfs] switch_root 到 Aevum 世代根 ...
[gen-init] 根标志(引擎产): Aevum generation root: gen-40
[gen-init] / 布局(来自世代 rel_path,非脚本手拼): AEVUM_GENERATION_ROOT bin dev lib lib64 ...
[gen-init] hello 直接跑: Hello, world!
[gen-init] Aevum 引擎驱动的世代 = 系统根。补真达成。
```

- bootroot 布局来自 `aevum export-bootroot 40`(引擎读世代层级 symlink,67 文件忠实重建)。
- hello 运行闭包(libc+loader)来自 `aevum export-rootfs hello`(引擎补闭包)。
- 根标志带 gen-40(可审计:此根来自哪个世代)。
- 脚本 `build-bootroot-initramfs.sh` 退回"只引导":调两个 aevum 命令拿引擎产物 + 组 switch_root initramfs,不再手拼内容。

### 与之前的本质区别

| | 阶段1/2(旧) | 本轮(补真) |
|---|---|---|
| bootroot 来源 | 脚本手 cp 拼 | **aevum export-bootroot(引擎读世代)** |
| 闭包来源 | 脚本 cp | **aevum export-rootfs(引擎补)** |
| 证明 | Linux 能从目录引导 | **Aevum 世代机制驱动引导** |
| 世代布局 | 扁平(丢 rel_path) | 层级(rel_path 贯穿) |

### 验证规模

全 workspace 真 Linux 全绿(milestone1-8 端到端 + 各 crate 单测,含 generation_refs 层级布局断言);rel_path 改动无回归。

### 诚实标注的边界

- 仍 QEMU、上游内核、busybox init(不变)。
- `export-rootfs` 补的是 hello 单包闭包;多包完整闭包入世代是后话。
- install 仍不补运行闭包(只装包文件),故 bootroot 的 libc 来自 export-rootfs 而非 install 的世代——install 与 build/闭包的统一是待办。
- 仍未做 bootloader 多世代菜单(阶段3)、真 init 系统/服务(阶段4+)。

---

## 2026-06-11(日)—— 第二十一轮:可引导阶段2,Aevum 世代成为系统真实根(switch_root)

阶段1 在 initramfs 临时根里跑 Aevum 软件;阶段2 用 `switch_root` 把 **Aevum 世代切成真实系统根**,进程1 移交给世代里的 /sbin/init。"Aevum 世代 = 系统"真正落地。

### QEMU 验收(串口铁证)

```
[initramfs] 准备 switch_root 到 Aevum 世代根 ...
 Aevum 阶段2: 已 switch_root 到【Aevum 世代真实根】
[gen-init] 当前 / 的根标志: this is an Aevum generation root
[gen-init] / 下内容: AEVUM_GENERATION_ROOT bin dev lib lib64 proc sbin sys tmp
[gen-init] hello 直接跑(靠世代根 /lib64 loader 软链,非注入): Hello, world!
[gen-init] Aevum 世代 = 这个系统的根。阶段2 达成。
```

- **switch_root 切到世代根**:进程1 从 initramfs 移交给世代根 /sbin/init。
- **根标志证明**:`/AEVUM_GENERATION_ROOT` 存在 → 当前 `/` 确是 Aevum 世代,非 initramfs。
- **hello 直接执行**:作为世代根 `/bin/hello`,靠世代根自带 `/lib64/ld-linux` 软链直接跑,**不再需要外部 loader 注入**——世代根布局自洽,像真正的根。

### 关键产出

- **scripts/build-bootroot-initramfs.sh**:组装 Aevum 世代真实根(busybox+hello闭包+/lib64 loader软链+/sbin/init+根标志)+ switch_root 跳板 initramfs。

### 实现期抓到并修的真 bug(没蒙混)

- 首次 `switch_root` → kernel panic(`Attempted to kill init`)。根因:switch_root 要求 NEW_ROOT 是**独立挂载点**且**会删光旧根**,而世代根放在 initramfs 子目录里自相矛盾(删 / 时把它自己删了)。修正:挂 tmpfs 作 newroot、`cp -a` 世代内容进去再 switch_root(tmpfs 是真挂载点)。

### 阶段1 vs 阶段2

| | 阶段1 | 阶段2 |
|---|---|---|
| 根 | initramfs(临时) | **Aevum 世代(switch_root 后真实根)** |
| hello | loader 注入跑 | 世代根 /bin/hello 直接跑(/lib64 软链) |
| init | initramfs /init | 世代根 /sbin/init(进程1 移交) |

### 诚实标注的边界

- 阶段2 = 世代当真实根,**仍不是完整发行版**:无 bootloader 多世代菜单(阶段3)、无真 init 系统/服务/系统配置(阶段4+)。
- 世代根经 tmpfs 承载(QEMU 无磁盘);真实场景应挂内容寻址 store 为只读根 + tmpfs 可写层,本轮简化。
- /sbin/init 是 busybox sh,非生产 init 系统。
- 仍 QEMU、仍上游内核(守 ADR-0001)。

### 下一步(阶段3+)

bootloader(extlinux/systemd-boot)多世代菜单 + 开机选世代 + 回滚;真 init 系统/服务管理;只读 store 根 + 持久化层。

---

## 2026-06-11(日)—— 第二十轮:可引导发行版阶段1,Aevum 在 QEMU 里真开机了

ADR-0006(可引导发行版增量项目,触发 ADR-0001 预设演进)立项后,阶段1 达成——**Aevum 世代作为系统根,在 QEMU 虚拟机里真引导起来,落到能用的 shell**。从"包管理器"迈向"可引导发行版"的第一块地基。

### QEMU 引导验收(串口日志铁证)

```
SeaBIOS → Linux version 6.12.86+deb13-amd64        ← Aevum install 的内核
[0.198643] Kernel command line: console=ttyS0 rdinit=/init
 Aevum bootable 阶段1: 内核已引导,init(进程1)接管
[init] 运行 Aevum store 里的 hello(loader 注入,PoC-2):
Hello, world!                                       ← Aevum 世代闭包真跑
[init] Aevum 世代成功作为系统内容被引导运行。阶段1 达成。
BusyBox v1.37.0 built-in shell (ash)
~ #                                                 ← 落到可用 shell
```

完整启动链 BIOS→内核→init(进程1)→Aevum 软件→shell 跑通,**每个关键件都来自 Aevum**:
- **内核**:`aevum install linux-image-6.12.86+deb13-amd64`(吃自己狗粮,4245 store 对象)。
- **init/shell**:`aevum install busybox-static`(静态,呼应 Foundation min-toolset 原则)。
- **运行的 hello**:Aevum 世代闭包,loader 注入(PoC-2)。

### 关键产出

- **scripts/build-initramfs.sh**:组装 initramfs——busybox(Aevum 装)做 /init 与兜底 shell + hello 闭包(Aevum 装),/init 挂 proc/sys → loader 注入跑 Aevum 的 hello → 落 busybox shell。cpio.gz 打包。
- **QEMU 引导**:`-kernel vmlinuz -initrd initramfs.cpio.gz -append "console=ttyS0 rdinit=/init"`,串口验证(scoop qemu 11.0.0,Windows 侧)。
- **环境准备**:内核/busybox 全由 Aevum install 从 Debian 镜像下载(阶段0)。

### 实现期抓到的真问题

- PowerShell 数组传 `-append` 把带空格的内核命令行拆断 → QEMU 误当文件名。修正:单字符串命令行 + 双引号包 append 值。
- `-nographic` stdout 重定向不实时 → 改 `-serial file:` 直写串口,抓全 372 行日志。
- tcg 软件模拟慢,首跑 35s 不够内核到 init;延长 + serial file 后完整跑通。

### 诚实标注的边界

- **阶段1 = 最小可引导**:证明 Aevum 世代能当系统根被引导,**不等于完整发行版**。
- **仍是 initramfs 内运行**:还没做"挂载世代为真实根 + 真 init 系统"(阶段2)、bootloader 多世代菜单(阶段3)、系统配置/服务(阶段4+)。
- **仍复用上游内核**(守 ADR-0001):打包 Debian 内核,不自造。
- **QEMU 验证,非物理机**:机制验证足够,真实硬件引导是后话。
- 内核含模块未精简(4245 对象);initramfs 暂用 busybox+单包,非完整 rootfs。

### 里程碑全景(用户态引擎 → 可引导)

里程碑1-8(用户态引擎:store/世代/求解/install/全裸容器) + AI 意图层 + 人在回路 + ADR-0006 阶段1(QEMU 真引导)。从"起 Rust 骨架"到"在虚拟机里用 Aevum 引导开机进 shell"。

### 下一步(ADR-0006 阶段2+)

挂载世代为真实根 + 真 init 接管;世代切换重建 initramfs/bootloader(含内核,重启生效);多世代引导菜单 + 回滚;系统配置/服务长尾。

---

## 2026-06-11(日)—— 第十九轮:里程碑8,全裸容器验证(FROM scratch,无包管理器/无系统库)

回答"能在没有其它包管理器的 Linux 里测吗"——用 **`FROM scratch` 全裸容器**做最硬的证明:除了内核(宿主共享)和 Aevum 装的 store,什么都没有(无 /bin、/usr、/lib、无 shell、无 apt)。

### 端到端验收(真 Docker scratch 容器)

```
rootfs 内容(全部): bin/hello  lib/ld-linux-x86-64.so.2  lib/libc.so.6
Dockerfile: FROM scratch + COPY rootfs + ENTRYPOINT loader 注入

对照1 裸跑 hello:        exec /bin/hello: no such file or directory   ← 失败(预期)
对照2 Aevum闭包+loader:  Hello, world!                                ← 成功
```

**决定性对照**:同一个 hello 二进制,直接跑失败(全裸环境无写死的 `/lib64/ld-linux`、无 libc),经 Aevum 导出的闭包 + loader 注入则打印 Hello, world!。→ 让它跑起来的是 **Aevum 装的 store(hello+libc+loader),不是环境自带任何东西**。

### 关键改动

- **cli `export_rootfs`**:把一个包的运行闭包导出成**自包含 rootfs**——复制实体文件(非 symlink 回 store,因 Docker COPY 会断链)到 `bin/<name>` + `lib/<soname>` + `lib/<loader>`;产 `run_argv = lib/<loader> --library-path lib bin/<name>`(PoC-2 loader 注入,不依赖写死 interp)。
- **cli `export-rootfs` 子命令**:build 闭包 → 导出目录 + 打印运行命令。
- **scripts/scratch-demo.sh**:阶段A(export-rootfs)+ 阶段B(FROM scratch docker build/run)+ 裸跑对照。

### 串起全链

resolve(意图/求解)→ install(从 Debian 镜像真下载)→ build(补闭包)→ export-rootfs(自包含)→ **全裸容器真跑**。从"起骨架"到"装的软件在零依赖环境跑起来"。

### 诚实标注的边界

- **证明的是"运行闭包自包含"**:单个 hello 在全裸跑通,不等于完整发行版(复杂服务需更多闭包/配置/maintainer scripts)。
- **仍复用宿主内核**(Docker 容器共享内核):不是从裸机引导 OS,符合 Aevum 用户态层定位。"从零搭建 Linux"超出设计范围。
- **loader 注入用 --library-path**(PoC-2),不改 ELF interp(保内容不可变)。
- 之前网络问题导致 registry 拉不动;`FROM scratch` 内置无需拉取,正好绕开。

### 验证规模

全 workspace 全绿(milestone1-7 + 各 crate 单测);里程碑8 经真 Docker scratch 容器验证(脚本,非 cargo test)。

---

## 2026-06-11(日)—— 第十八轮:里程碑7,resolve→install 全链打通(真能装了)

补上最大断桥:此前 `resolve` 算出 lock(包名+版本+filename),但 `build` 吃手工解包的目录,两者没接起来。本轮让 `aevum install` 真的从 Debian 镜像**下载→校验→解包→入 store→造世代**。Aevum 第一次"真能装一个包"。

### 端到端验收(`crates/cli/tests/milestone7.rs`,真下载)

```
resolve: hello 2.10-5 filename=pool/main/h/hello/hello_2.10-5_amd64.deb
install: 装 ["hello"] → gen-7 (49 store 对象)
```

- 用真实小包 `hello`(53KB,Debian 经典最小包)`--only` 单装。
- **真下载**:curl 从 `deb.debian.org` 拉 .deb。
- **SHA256 内容寻址校验**:下载内容 hash == lock 记录的 sha256,失配拒绝(供应链命脉)。
- **解包**:ar 取 data.tar.xz → tar 解(复用系统工具,不引 Rust 依赖)。
- **入 store + 造世代**:ingest_dir 内容寻址入库(49 对象)→ make_generation(7) + 激活。
- 验证解包目录真有 `/usr/bin/hello` 二进制(= 内容来自镜像、SHA256 已校验未被污染)。

### 关键改动

- **cli lib**:`download_deb`(curl + SHA256 校验 + 幂等)、`unpack_deb`(ar/tar,cfg unix)、`install` 编排(下载→解包→ingest_dir→世代);lock 行加 filename(`name@version#fingerprint\t filename`)。
- **cli main**:`install <pkg> [--only] [--mirror] [--generation]` 子命令,resolve→摊开将装的包→install。
- 沿用"调系统工具不引依赖"套路:curl/ar/tar/xz;SHA256 用已有 sha2。

### 实现期抓到的真 bug(没掩盖)

- 首跑 `tar 解 data.tar 失败`:.deb 的 data.tar 是 **.xz 压缩**,WSL 缺 `xz`。原测试把 install 失败一律当网络问题 skip,**会掩盖真 bug**。修正:(1) 装 xz-utils 并加进前提检查;(2) 改测试——只对"下载/网络"类错误 skip,解包/校验失败必须 panic 暴露。

### ⚠️ 运行时新增系统依赖

install 需要宿主有 `curl`/`ar`(binutils)/`tar`/`xz-utils`(+ `zstd` 若遇 data.tar.zst)。这些是 .deb 解包的现实依赖,与"不引 Rust 依赖"不冲突(调系统工具)。

### 诚实标注的边界(重要,别高估)

- **不执行 maintainer scripts**(postinst/preinst):装出的是"文件就位"的环境,**非"配置完成"**。hello 这类纯文件包够用;需 postinst 的包是半成品。
- **默认装 lock 全集,建议 --only 限定**:本轮验证用 `--only hello` 单装,没真装 455 包大闭包(过重 + 半成品问题放大)。
- **仍是用户态层**(OVERVIEW 边界):装进 store+世代,不碰宿主系统目录,**不是从裸机装 OS**。"从零搭建 Linux"超出 Aevum 设计范围(它复用宿主内核)。
- store→PATH 可用环境的"激活生效"仍未做(装进 store 了,但没有"激活后命令行就能用 hello")。

### 验证规模

全 workspace 全绿:milestone1-7 端到端 + 各 crate 单测。SHA256 校验、解包入库、世代激活经真下载固化。

---

## 2026-06-11(日)—— 第十七轮:AI 增强层落地,意图→约束→确定性求解端到端

第十六轮把确定性核心(solver)端到端打通后,本轮落地 ADR-0003/0005 的 **AI 增强层**——并先出实现设计(`docs/ai/05-intent-resolver-implementation.md`)再实现。核心:AI 是 solver 的**前置意图适配器**,把模糊意图翻译成约束,不碰任何已验证的确定性逻辑。

### 端到端验收(真 DeepSeek + 真实 Debian 索引)

```
意图="我要数据科学环境" → DeepSeek 翻译 → r-base,python3-pandas,python3-numpy,
   python3-scipy,python3-sklearn,jupyter-notebook,... (12 个真实 Debian 包)
   → 确定性求解(真实 6.8 万包索引)→ closure_id=clo-4e5109e5815c0697 (455 包,0 未解析)
```

`crates/cli/tests/milestone6.rs` 3 测(Mock 主验证 + 离线降级 + 真 DeepSeek):

- **AI 真翻译**:DeepSeek 把一句中文意图翻译成真实 Debian 包名(数据科学→pandas/numpy/scipy/sklearn/jupyter/r-base)。
- **可复现不依赖模型(ADR-0005 命脉)**:同一批约束,走 AI 翻译 vs 直接喂包名,**closure_id 字节级一致**。把模型拿掉、用 lock 约束重放得到分毫不差的世代——可复现来自确定性闭包,不来自模型。
- **ai_assist 可审计(ADR-0003 边界3)**:lock 记录 `involved=true model=deepseek-chat reason=...`,但重放只用确定性部分。
- **离线降级(ADR-0005)**:无匹配意图 Err,Explicit/Mock 透传,确定性核心仍可用。

### 关键改动

- **新增 `intent` crate**:`IntentResolver` trait + `Intent`(NaturalLanguage/Template/Explicit)/`IntentOutcome`/`AiAssist`;`MockIntentResolver`(规则映射,离线确定性,CI 可跑)+ `DeepSeekResolver`。
- **DeepSeekResolver 经系统 `curl` 调 API**——**不引 Rust HTTP/JSON 依赖**(沿用 zstd/gunzip 的"调系统工具"套路):手工构造 JSON 请求体、极简提取 content;提示模型只回逐行包名;key 走 `DEEPSEEK_API_KEY` 环境变量(不硬编码、不入库)。
- **cli**:`resolve_intent` 编排(意图→翻译→确定性求解);`resolve` 子命令加 `--intent`/`--mock`;lock 写 `ai_assist` 行。

### 严格落在 ADR-0003 三边界内

- 边界1(不直接选 hash):AI 只产约束,closure_id 由确定性求解器算——已用"AI/无AI 路径 closure_id 一致"验证。
- 边界2(不动 Foundation):intent crate 不依赖 store/generation/closure-builder,只产约束喂 solver。
- 边界3(人类可否决):决策写 ai_assist 供审计(询问列表是后续 UI 层)。

### 验证规模

全 workspace 全绿:milestone1-6 端到端 + 各 crate 单测(intent 6 测新增)。AI/无AI 路径 closure_id 一致经测试固化。

### ⚠️ 安全提醒(运维事项,非代码)

本轮调试中 DeepSeek API key 曾以明文出现在开发会话里,**应轮换/作废该 key**。代码侧已确保 key 只经环境变量传递、不写入任何文件、不入 lock/库。

### 诚实标注的边界

- **真 LLM 接入与"vendor 离线、不引依赖"有张力**:DeepSeekResolver 经 curl 调用(运行时依赖系统 curl + 网络),与离线约束是有意权衡;离线/CI 用 MockIntentResolver。
- DeepSeek 响应提取是极简字符串处理(找 content 字段),非完整 JSON 解析——够用且无依赖,但对异常响应不够健壮(失败则 Err 降级)。
- 模板(Template intent)未实现(模板 crate 待建)。

---

## 2026-06-11(日)—— 第十六轮:里程碑5,solver 接真实 Debian 索引端到端

补上唯一空白:**solver 此前从未端到端验证**(12 单测全手构造小索引,cli `resolve` 占位)。solver 是 ADR-0003 核心(AI 产意图、确定性求解器算闭包并产 lock,可复现只来自 lock),但一直没用真实数据跑过。本轮用真实 Debian 索引兑现。

### 端到端验收(`crates/cli/tests/milestone5.rs`,跨平台)

```
解析真实索引: coreutils 闭包 15 包, closure_id=clo-58e5cd36684a7802, 未解析 0
可复现验证: 两次求解 closure_id 一致 = clo-58e5cd36684a7802
```

- 真实数据:`poc/poc1-index-feasibility/data/Packages.gz`(~6.8 万包条目,PoC-3 用过)。
- **传递闭包正确**:coreutils 解出 15 包闭包,含 libc6,0 未解析。
- **可复现铁律(PoC-3)**:同输入两次求解 closure_id 字节级一致;且 **Windows 与 WSL 跨平台同一 closure_id**(`clo-58e5cd36684a7802`)——确定性不依赖平台,无随机/时钟/AI。

### 关键改动

- **solver**:`Index::from_packages_str`(直译 PoC-3 `load_index`:段落格式/Provides 虚包/续行跳过)+ `package_count`;3 个新单测(解析/虚包/解析→求解可复现)。
- **cli**:`Layout` 加 index_file/locks_dir;`resolve` 编排(读真实 Packages → solver::resolve → build_lock → 写纯文本 lock);`resolve` 子命令去占位(支持多包名 + lock 命名)。
- **scripts/prep-index.sh**:系统 gunzip 解压索引(不引 flate2,同 zstd 策略)。

### 验证规模

全 workspace **47 测全绿**(milestone1-5 端到端 1+1+4+2+1 + solver 15 + closure-builder 13 + store 6 + generation 2 + cli-lib 2)。真 Linux + Windows 双跑,closure_id 跨平台一致。

### 至此的完整图景

六个核心 crate 全部端到端真跑通,无纯骨架核心逻辑:

| crate | 端到端验证 |
|---|---|
| solver | coreutils 解真实 6.8万包索引,closure_id 可复现(里程碑5)|
| closure-builder | rg/python/im 补闭包,扫 81/143 ELF(里程碑1-4)|
| store | 内容寻址 + setuid + symlink + 整目录入库(里程碑1-3)|
| generation | 原子切换/回滚/GC 真 symlink(里程碑1)|
| elf | rg/python/im 真 ELF 解析 |
| cli | resolve/build/switch/rollback/gc + 轻隔离运行 |

### 诚实标注的剩余裁剪

- 续行字段解析简化(PoC-3 同款,真实索引依赖极少跨行)。
- override(pin/exclude)机制在 solver 已有,cli resolve 暂只暴露基础 template 求解。
- AI 维护层未碰(符合 ADR-0003:求解不需要 AI;AI 是另一条增强线)。
- 多源路由仍是机制演示(无真实多源数据,里程碑4 标注)。

---

## 2026-06-11(日)—— 第十五轮:里程碑4 达成,im 真转图 + 外部依赖 + 同源硬约束

里程碑3 最大遗憾(im 真转图未成)补上。三块(用户全选)全绿(`crates/cli/tests/milestone4.rs` 2 测 + closure-builder 块3 单测)。

### 三块成果

- **块1+2 外部依赖纳入闭包 + im 真转图(里程碑3 遗憾补上)**:
  - 装齐核心 delegate(liblcms2/libjpeg/libpng16/libfreetype/liblqr/libraqm/libfontconfig/libglib/libX11 等)后,im 闭包解出 **43 库**(此前 11),核心 delegate 全部纳入。missing 从 34 降到 20(全是冷门格式 heif/jxl/exr/raw,不影响 PNG)。
  - **真转图 ✅**:轻隔离 `magick -size 32x32 xc:red out.png` → **rc=0,产出 313 字节合法 PNG**(魔数 `\x89PNG` 校验通过)。137 coders + delegate dlopen 闭包真闭合——这是里程碑3 受阻于宿主缺库、本轮补齐后的兑现。
- **块3 同源硬约束(PoC-4 铁律落地)**:
  - `SourcePolicy{Lenient,Strict}`;`build_closure_resolved_with_policy` Strict 模式遇跨源 → `Err(CrossSource)` 硬阻断。默认 Lenient 保里程碑1-3 兼容。
  - 真验证:rg(Arch 包)的 libpcre2/libc 等来自 Debian 宿主 = 跨源。Lenient 记 4 条诊断;**Strict 硬阻断**(libpcre2-8.so.0 来自 Debian ≠ Arch)。从里程碑3 的"诊断可见"升级为"可阻断"。
  - `SourceRoutedResolver`:按 soname→源 路由到对应源 resolver(多源机制,构造数据单测)。

### 实现期抓到的真实工程问题

- **跨发行版 ABI(libxml2.so.16 vs .so.2)**:Arch 的 magick 链接 `libxml2.so.16`,Debian 只有 `libxml2.so.2`(同库不同 soname 版本号)。正是 PoC-4"坑在 ABI 不在路径"的真实案例。处理:不污染宿主,在项目 `.aevum/abi-bridge/` 建 `libxml2.so.16 → 宿主 .so.2` 软链,经 `--library-path` 提供。诚实标注为"跨源版本号桥接",真实多源场景应走同源库。
- **magick 直接 NEEDED 链比预想长**:liblcms2/liblqr/libraqm/libfontconfig 等都是 libMagickCore 直接 NEEDED(启动必需,非可选 dlopen)。用 `ldd` 一次性定位缺失库,避免逐个试错。

### 关键改动

- **closure-builder**:`SourcePolicy`;`build_closure_resolved_with_policy`(Strict 硬阻断,旧 `build_closure_resolved` 委托 Lenient);`SourceRoutedResolver`(多源路由机制)。
- **scripts/prep-im-delegates.sh**:装核心 delegate + 建 abi-bridge 软链。
- **.gitignore**:加 `/.aevum`(运行期状态可重建)。

### 验证规模

全 workspace 真 Linux **43 测全绿**(milestone1 端到端 1 + milestone2 端到端 1 + milestone3 端到端 4 + milestone4 端到端 2 + solver 12 + closure-builder 13 + store 6 + generation 2 + cli-lib 2);Windows 侧 cfg 守卫下全绿。

### 诚实标注的裁剪(留待真实数据/未来)

- **多源路由是机制演示**:本仓库只有 3 个 Arch 包,无 Nix/Debian 同名包数据,`SourceRoutedResolver` 用构造数据验证路由机制,**非真实多源数据验证**——真实验证需带同名包的多源数据。
- **冷门格式 delegate 不装**(heif/jxl/exr/raw/cairo/pango 等 20 个),对应 coder 仍 missing,不影响 PNG/基础格式转图。
- **libxml2 是版本号 ABI 桥接**,非真正同源解决;真实场景应从 magick 所在源取配套 libxml2。

### 里程碑全景(1-4 递进完成)

1. rg 简单包闭环(补闭包→入库→世代→轻隔离运行→回滚→GC)
2. 真 Linux 验证 unix 语义(原子切换/setuid)
3. python dlopen 闭包(PoC-5 盲区:import ssl 不崩)
4. 复杂包工程化(自动推断/store 视图/同源诊断)+ im 137 coders 真转图 + 同源硬约束

---

## 2026-06-10(六)—— 第十四轮:里程碑3 达成,复杂包工程化收口

里程碑1/2 的有意裁剪在本轮收口,并加最复杂包(imagemagick 137 coders)验证。四块全做,真 Linux 端到端(`crates/cli/tests/milestone3.rs`,4 块全绿)。

### 四块成果

- **块1 自动推断 runtime_dir**:`infer_runtime_dirs` 布局启发式扫描——含 `lib-dynload`/`coders`/`modules` 关键词或 ≥5 个 `.so` 的目录,上溯到 `usr/lib/<X>` 运行时根。对真包推断:python→`usr/lib/python3.14`、im→`usr/lib/ImageMagick-7.1.2`。免去手填 `--runtime-dir`。
  - **修正原设想**:Arch `.PKGINFO` 无路径字段(实测确认),只有 pkgname/provides/depend,所以靠包内布局推断而非读元数据路径。
- **块2 imagemagick**:扫 **143 ELF**(主+136 coders+核心库),解出 libMagickCore 等,dlopen 闭包算法验证通过。真转图受阻于**宿主缺 34 个 delegate 库**(liblcms2/libjpeg/libpng/libfreetype 等,既不在包内、宿主 WSL 也未装,部分是 libMagickCore 直接 NEEDED)——这是宿主环境缺依赖,非补闭包算法缺陷,`missing_libs` 完整列出。按计划真转图作 stretch。
- **块3 从 store 重建运行视图**:`materialize_view` 按 rel_path symlink 回 store 对象重建包内布局(软链保留不复制,PoC-5)。**真验收**:把 python 标准库从 store 重建到 `<view>/lib/python3.14`,PYTHONHOME 指向视图跑 `import ssl` → **rc=0、OpenSSL 3.5.4**。证明内容寻址 store + 布局重建 = 可运行真相源,不依赖解包目录。
- **块4 同源校验收紧(诊断可见)**:`LibResolver` 加 `provenance`/`resolve_with_provenance`,`HostLibResolver`=Debian、`PackageLibResolver`=包源、`ChainResolver` 透传命中源。`build_closure_resolved` 把"库来自异于包 source 的源"记入 `closure.cross_source`(诊断,**不硬阻断**,保里程碑1/2 不炸)。rg(Arch)的库从 Debian 宿主取 = 跨源,可观测。PoC-4 铁律从"静默放宽"变"显式可见"。

### 关键改动

- **closure-builder**:`infer_runtime_dirs` + `runtime_root_of`;`Closure` 增 `cross_source: Vec<CrossSourceHit>`;`LibResolver` 增 `provenance`/`resolve_with_provenance` 默认方法;`PackageLibResolver` 增 source 字段;`ChainResolver` 透传命中源。
- **cli**:`build_with` runtime_dirs 空时自动推断;`materialize_view`(cfg unix);`BuiltClosure` 增 included_dirs/scanned_elf_count。
- **scripts/prep-complex.sh**:解 py/im。

### 验证规模

全 workspace 真 Linux 39 测全绿(milestone1 端到端 1 + milestone2 端到端 1 + milestone3 端到端 4 + solver 12 + closure-builder 11 + store 6 + generation 2 + cli-lib 2);Windows 侧 cfg 守卫下全绿。

### 诚实标注的裁剪(里程碑4 收口)

- im 真转图:需宿主装齐 34 个 delegate 库,或把 delegate 也补进闭包(多源/外部依赖纳入)。本轮以补闭包完整性(验证A)为强验收。
- 同源校验:本轮做到"诊断可见"(cross_source 报告),**硬阻断 + 多源 store 路由**留里程碑4。
- runtime_dir 推断用布局启发式;接 nixpkgs 带路径元数据的源可更精确。

### 下一步(里程碑4 候选)

多源 store 路由 + 同源硬约束;外部 delegate 依赖纳入闭包(im 真转图);.PKGINFO depend 字段用于源路由。

---

## 2026-06-10(六)—— 第十三轮:里程碑2 达成,复杂包 dlopen 闭包(PoC-5 盲区已补)

PoC-5 发现 PoC-4 的"只递归主二进制 NEEDED"算法对复杂包**不成立**:python 主二进制 NEEDED 只有 libpython/libc,但 `import ssl` 运行时 dlopen `lib-dynload/_ssl.so` → 再 NEEDED libssl/libcrypto,整条链 ELF 静态分析看不见。里程碑2 用真实 python 包验证 Aevum 把这条链补回来了。

### 端到端验收(`crates/cli/tests/milestone2.rs`,WSL 真 Linux)

```
验证A: 扫 81 ELF, 解出 19 库(含 libssl/libcrypto), 缺失 5 个
验证B: import ssl rc=0, OpenSSL 3.5.4
里程碑2 达成: 复杂包 python 补全 dlopen 闭包(PoC-5 盲区已补),import ssl 不崩
```

- **验证A(补闭包完整性)**:对真 python 跑 `build_with`,扫到 81 个 ELF(主+77 lib-dynload+libpython 等,非只主二进制)。决定性断言——`resolved_libs` 含 `libssl.so`/`libcrypto.so`,这俩是 `_ssl.so` 的传递依赖、主二进制 NEEDED 看不见,**正是 PoC-4 盲区,被"扫全包 ELF + 递归"补回**。
- **验证B(真跑,强验收)**:轻隔离 `python3 -c "import ssl"` → **rc=0 并打印 OpenSSL 3.5.4** = dlopen 闭包闭合,直接证伪 PoC-5 的崩溃场景。
- 缺失 5 个(libgdbm/libmpdec/libtcl/libtk)是正确的成本信号:`_gdbm`/`_tkinter` 等可选扩展依赖,宿主与包内都无,`import ssl` 不触及。

### 关键改动

- **closure-builder**:新增 `PackageLibResolver`(包内库优先,libpython 在包里宿主没有)+ `ChainResolver`(包内→宿主按序兜底,PoC-5)。`build_closure_resolved` 签名未改(已是 `&dyn LibResolver`),组合 resolver 注入即可。
- **cli**:`build_with(runtime_dirs)` 支持复杂包,包内库搜索路径自动含 `usr/lib` + 各 runtime_dir 及其 `lib-dynload`。`ingest_closure` 用 `store.ingest_dir` 把运行时目录(标准库 .py + lib-dynload + 软链)整体内容寻址入库(源3/4),软链保留不翻倍(PoC-5)。`aevum build --runtime-dir` 暴露给 CLI。
- **scripts/prep-complex.sh**:系统 zstd 解 py/im 包。

### 里程碑2 的有意裁剪(诚实标注)

- **imagemagick 转图留作 stretch**:以 python `import ssl` 为强验收(更干净地证 dlopen 闭包)。
- **runtime_dirs 暂由调用方/`--runtime-dir` 指定**,不解析 .PKGINFO 自动推断(留里程碑3,避免引 ini 依赖)。
- 从 store 重建整目录视图运行留作 stretch:验证 B 用解包目录作运行视图 + store 解出的库证 dlopen 闭包完整性。

### 验证规模

全 workspace 真 Linux 30 测全绿(milestone1 端到端 1 + milestone2 端到端 1 + solver 12 + store 6 + closure-builder 6 + generation 2 + cli-lib 2);Windows 侧 cfg 守卫下全绿。依赖经 vendor 离线编译。

### 下一步

里程碑3:接 .PKGINFO 自动推断运行时目录 + 真实多源同源校验收紧(里程碑1/2 的同源放宽收口);imagemagick 137 coders 转图验证;从 store 重建运行视图。

---

## 2026-06-10(六)—— 第十二轮:里程碑1 达成,装一个 rg 并回滚

kickoff §3 的里程碑1 跑通——把 closure-builder/store/generation 串成第一个**可工作闭环**,用真实 Arch `rg` 包在真 Linux 完成"补闭包→入库→造世代→轻隔离运行→回滚→GC"。跑通即 Aevum 核心机制不再是纸面,而是端到端可复现。

### 端到端 6 步全绿(`crates/cli/tests/milestone1.rs`,WSL 真 Linux)

```
里程碑1 达成: rg 装→跑(rc0)→切 gen-2→回滚 gen-1→GC(回收1,保留6)
```

- **补闭包解齐依赖**:rg 的 libc/libgcc_s/libpcre2 等从宿主解齐,0 缺失,PT_INTERP loader 解出。
- **内容寻址入库**:7 个对象,每个加载期 hash 校验通过。
- **轻隔离运行 ✅(PoC-2)**:`<store内ld-linux> --library-path <store库目录们> rg --version` → rc=0、输出含 "ripgrep",**不依赖宿主 /lib**、不改 ELF。
- **原子切换+回滚 ✅(PoC-7)**:gen-1→gen-2→回滚 gen-1。
- **可达性 GC ✅(PoC-7)**:只保留 gen-1 时回收 gen-2 独占的 marker-rg(1),共享库(6)全保留不误删。

### 各 crate 从骨架到接通

- **store**:新增 `put_symlink`(symlink 入库不解引用,PoC-5)、`ingest_dir`(整目录内容寻址)、`get` 加载期 hash 校验。
- **closure-builder**:新增 `HostLibResolver` + `build_closure_resolved`——BFS 递归解 soname→真实库文件 + interp 解析(PoC-4)。初始队列=全包 soname 不退化(PoC-5)。
- **cli**:lib 化,新增 `run_isolated`(PoC-2 loader 注入)+ `$AEVUM_ROOT` 路径派生 + build/switch/rollback/gc 子命令真接通。
- **scripts/prep-rg.sh**:系统 zstd 解 Arch 包(不引 Rust zstd 依赖)。

### 实现期抓到的两处真 bug

- **mode 文件类型位污染 hash**:`read_meta` 返回 `permissions().mode()` 含 `S_IFREG`(0o100000),与入库时传入的纯权限 mode 不一致 → put/get 往返假 HashMismatch。修为 `& 0o7777` 只取权限语义。Windows 侧因 read_meta 走 Unsupported 未暴露,真 Linux 全量跑才现形。
- **get 加载期校验破坏跨平台**:校验依赖 unix mode,非 unix 必失败。修为 `cfg(unix)` 守卫,非 unix 仅校验存在。

### 里程碑1 的有意简化(里程碑2 收紧)

- **同源放宽**:rg 是 Arch 包但库从 Debian 宿主取,`HostLibResolver` 把宿主当唯一源、不做 CrossSource 检查(代码显式标注)。接真实多源时按 source 路由 + 恢复同源校验(PoC-4 铁律)。
- gen-2 用"对 rg 追加无害 marker 造不同 hash"模拟第二版本(单包无真实第二版本)。

### 验证规模

全 workspace 真 Linux 27 测全绿(milestone1 端到端 1 + solver 12 + store 6 + closure-builder 4 + generation 2 + cli-lib 2);Windows 侧 cfg 守卫下全绿。依赖经 vendor 离线编译。

### 下一步

里程碑2:复杂包(python/imagemagick)——扫全包 ELF 的 dlopen 闭包、运行时目录整体纳入、真实多源同源校验收紧。

---

## 2026-06-10(六)—— 第十一轮:Rust 骨架在真 Linux 验证 unix 语义

第十轮的骨架此前只在 Windows 侧 `cargo build/test`,关键的 unix 语义(symlink+rename 原子切换、setuid 权限位往返)被 `#[cfg(unix)]` 跳过。本轮在 WSL Debian 13(真 Linux)把这些测试真正跑通。

### 验证结果(真 Linux,20+ 测试全过)

- **`generation::atomic_switch_and_rollback` ✅**:真 symlink + `rename` 的原子切换/回滚,实证 PoC-7 机制(之前在 Windows 被 cfg 跳过)。
- **`store::setuid_bit_survives_roundtrip` ✅(新增)**:put 一个 0o4755 文件后从 store 读回,setuid 位仍在——实证 PoC-6 铁律(权限位不显式恢复则 sudo 提权失效)。原仅注释,本轮落成真实 `#[cfg(unix)]` 测试。
- solver 12 测、store 4 测、generation 2 测、closure-builder 2 测、elf 2 测全过。

### 构建环境与一处工程决策

- 工具链:WSL Debian 13 + rustup(国内 rsproxy 镜像)+ gcc 14.2。
- **离线 vendor 方案**:WSL 镜像网络模式间歇断连导致 `cargo` 拉依赖失败。改为在联网的 Windows 侧 `cargo vendor vendor` 导出依赖,加项目级 `.cargo/config.toml` 指向 `vendor/`,WSL 经 `/mnt/d` 离线编译。`vendor/` 与 `target/` 已加入新建的 `.gitignore`(不入版本控制,可重新生成)。
- 真 Linux 测试用 `CARGO_TARGET_DIR=/tmp/aevum-target` 与 Windows 侧 `target/` 隔离(目标三元组不同)。

### 下一步(承上轮里程碑1)

unix 语义已实证可靠,可放心串 solver→closure-builder→store→generation 实现"装一个 rg 并回滚"的最小闭环。

---

## 2026-06-09(五)—— 第十轮:Rust workspace 骨架落地(设计→实现首步)

按 [`guides/01-rust-implementation-kickoff.md`](guides/01-rust-implementation-kickoff.md) 起 Rust workspace,把 7 个 PoC 校正过的算法落成第一版可 `cargo build` 的代码。

### 产出(6 个 crate)

- `crates/elf`:goblin 解析 PT_INTERP/DT_NEEDED/SONAME/RUNPATH;`scan_dir` 扫全包 ELF 且不解引用 symlink(PoC-5)。
- `crates/solver`:Debian 版本比较 + 确定性闭包求解 + closure_id,**直译自 PoC-3 solver.py**;12 单测含「closure_id 三次一致」「最大满足版本」「exclude 留痕」。
- `crates/store`:内容寻址 sha256,**权限位纳入哈希输入 + 显式恢复**(PoC-6),symlink 保留(PoC-5);unix 语义用 `cfg(unix)` 守卫。
- `crates/generation`:原子切换(symlink+rename)/瞬时回滚/可达性 GC,**直译自 PoC-7**;GC 不误删共享依赖的断言为纯计算测试,跨平台可跑。
- `crates/closure-builder`:四源合一补闭包**框架**(主二进制 NEEDED / 全包 ELF / 运行时目录 / 数据目录),结构上强制四源避免退化;深度递归与同源校验标 TODO(里程碑1/2)。
- `crates/cli`:`aevum` 命令骨架(resolve/build/switch/rollback/gc),clap 驱动。

### 实现期抓到并修正的两处算法缺陷

- **Debian `~` 预发布序**:PoC 的 `_cmp_nonnum` 把整段映射成列表再比,但空列表恒为最小,无法表达「`~` 比段尾还小」。改为 dpkg 规范的逐位带哨兵比较(缺位权重 0,`~` 为 -1)。
- **exclude 诊断缺口**:被排除的**传递依赖**原静默跳过,`excluded_hit` 无留痕。补为展开处也记录(去重),使用户的 exclude 有可见反馈。

### 验证

- 工具链 cargo 1.94;`cargo build` 通过,`cargo test` 20 测全过,`aevum --help`/`resolve` 可运行。
- 真 Linux 行为(symlink+rename 原子切换、setuid 往返、真实 ELF/复杂包补闭包)的测试已写但留待 WSL/真 Linux 跑(Windows NTFS 不支持,见 CLAUDE.md)。

### 下一步

里程碑1「装一个 rg 并回滚」:载入真实索引串起 solver→closure-builder→store→generation;在 WSL 验证 unix 语义测试。

---

## 2026-06-09(五)—— 第九轮:PoC-7 核心机制实测(最关键一轮)

首次用真文件+真 symlink 验证 Aevum 立身之本——世代原子切换、瞬时回滚、GC 引用计数。

### PoC-7 结果(三大核心卖点全过 + 一个边界)

- **原子切换 ✅**:active symlink + `os.rename` 原子替换,0.09ms,无半切状态。
- **瞬时回滚 ✅**:指针回指旧世代,0.095ms,不重建。"秒回"实为"亚毫秒回"。
- **GC 引用计数 ✅(最关键)**:两世代共享 libc,删 gen-2 后只回收 py3.12,**共享 libc 正确保留未误删**。GC 最易写错的点实测正确。
- **强隔离 setuid 边界 ⚠️**(回答 PoC-6 待办):user-ns 内伪 root,真 setuid 提权受 `no_new_privs` 限制 → 系统级特权包(sudo)走专门授权通道,不靠沙箱内 setuid。
- 产出:`poc/poc7-core-mechanics/`(experiment.py + REPORT.md)。

### 调整

- `foundations/02-generation` §4:加原子切换/回滚 PoC-7 实证。
- `ai/03-garbage-collection` §1:加 GC 不误删共享依赖的 PoC-7 实证。
- `foundations/05`:加轻隔离不隔通信(PoC-6)+ 强隔离 setuid 边界(PoC-7)。

### 本轮设计共识

26. 原子世代切换、瞬时回滚、安全 GC 三大核心机制,真文件实测全部成立。
27. 强隔离沙箱内 setuid 不提权,特权系统组件走 System 层+显式授权,不靠沙箱 setuid。

### PoC 全景(7 个,核心假设全覆盖)

| PoC | 验证 | 结论 |
|---|---|---|
| 1 | 包索引能否机器生成 | 仅 11.6% → 继承上游 |
| 2 | 二进制开箱即跑 | 困境下 loader 救活 |
| 3 | 零 AI 求解 | 442 包零未解析可复现 |
| 4 | 多源隔离消费 | 可行,铁律=同源补闭包 |
| 5 | 复杂包补闭包 | DT_NEEDED 不够,须扫全包+元数据 |
| 6 | setuid/通信/磁盘 | setuid 须显式恢复;通信通;去重省88% |
| 7 | 世代/回滚/GC | 三大核心卖点真文件全过 |

---

## 2026-06-09(四)—— 第八轮:PoC-6 三个架构盲区压测

测 PoC-4/5 没碰的三个盲区:setuid 权限、多包通信、多版本磁盘代价。

### PoC-6 结果

- **setuid(抓到 bug 隐患)**:内容寻址天真 read→write 复制会丢 setuid 位,实测 sudo `0o4755`→`0o644`,提权失效。→ store 必须显式恢复权限位 + 语义权限位(可执行/setuid/setgid/sticky)纳入哈希输入,不可一律归一。
- **多包通信(验证假设)**:轻隔离只隔库搜索路径,不隔进程/exec/pipe,隔离包能正常互调(实测 rc=0)。默认轻隔离不破坏协作;强隔离才需打洞。
- **磁盘代价(验证假设)**:10 包各带同源闭包,同版本 glibc 内容寻址去重省 87.8%。磁盘可控;多版本占用是有意代价,GC 回收。
- 产出:`poc/poc6-arch-edges/`(experiment.py + REPORT.md)。

### 调整

- `foundations/01-store`:修正"归一权限位"为"语义权限位纳入哈希、只归一 mtime 噪声";新增"入库/取出显式恢复权限位含 setuid"。验收清单同步。

### 本轮设计共识

24. 语义权限位(可执行/setuid/setgid/sticky)是内容寻址的一部分,纳入哈希;入库取出显式恢复,setuid 不丢。
25. 轻隔离不隔进程通信(默认协作无碍);磁盘靠同版本去重压制,多版本占用有意且可 GC。

### 待办

- setuid 包在 user-namespace 强隔离下的提权行为未实测(namespace 内 setuid 受限)。
- 强隔离 namespace 完整 PoC 仍未做。

---

## 2026-06-09(三)—— 第七轮:PoC-5 复杂包压测,校正补闭包算法

压测复杂包(python 3.14、imagemagick 7.1),专踩 PoC-4 简单包算法的盲区。

### PoC-5:复杂包补闭包(找到算法缺陷)

- 发现:**"只递归主二进制 DT_NEEDED"对复杂包不完整**。python 主二进制 NEEDED 仅 2 个,但运行时还需 77 个 dlopen 扩展(`import ssl` → `_ssl.so` → libssl/libcrypto,主二进制完全看不见)+ 写死路径的标准库;imagemagick 有 137 个 dlopen 编解码插件。照 PoC-4 算法补闭包,这类包能启动但 import/打开格式即崩。
- 不是死局:算法可校正 = 主二进制 NEEDED 递归 + 扫全包所有 ELF + 纳入上游元数据声明的运行时目录/插件路径 + 整目录纳入数据路径。
- 附带发现:**符号链接是 store 一等公民**(复杂包大量用:137 个 magick 软链、python 版本链),规范化须保留不解引用。
- 再次印证"必须继承上游元数据":运行时结构(标准库目录/插件路径)只有上游标了,ELF 反推不出。
- 产出:`poc/poc5-complex-pkg/`(REPORT.md + 数据)。

### 调整

- `foundations/05`:新增 §3.5 复杂包 DT_NEEDED 不完整 + 校正后的四步补闭包算法;导入流程图、验收清单同步更新。
- `foundations/01-store`:规范化条款新增"符号链接保留不解引用"。

### 本轮设计共识

22. 补闭包对复杂包须四源合一:主二进制 NEEDED 递归 + 全包 ELF + 元数据运行时目录 + 数据路径。
23. 符号链接是内容寻址的一部分,保留不解引用。

### 待办

- setuid 包补闭包未测。
- 同源补闭包需拉取上游完整依赖树(只拉单包不够,复杂包尤甚)。
- 强隔离 namespace、多包通信边界未做 PoC。

---

## 2026-06-09(二)—— 第六轮:生态战略 + 多源消费 + PoC-4

围绕"能不能直接用 Nix/Arch 等现有生态、隔离式多包并存"做了战略调研(workflow)+ 实测(PoC-4),确立"消费现有生态而非自造"的落地机制。

### 战略结论(workflow 调研)

- 核实事实:Nix 包自包含(RUNPATH 指向自身闭包)、Nix store 可在任意 Linux 跑;Arch/Debian 包非自包含(靠标准路径)。
- 已有先例:devbox/flox(无 Nix 语言的 Nix 体验层)、Bedrock(混多发行版但放弃统一求解)、nix-portable/nix-user-chroot(免 root 跑 Nix)。
- 定调:**Aevum 的创新在"AI 维护 + 世代回滚 + 多版本并存",生态直接消费上游;不自造生态。**

### PoC-4:Arch 包补闭包 + 轻隔离(真实 Linux 实测)

- 真实 Arch ripgrep 包,递归补闭包(全自动零缺失),遮蔽标准库后:裸跑 rc=127,Aevum 轻隔离 rc=0(输出 ripgrep 15.1.0)。
- **关键发现**:多源最大的坑不在"路径"(隔离能解),在"ABI 兼容"——给 Arch 包喂 Debian 库报 "no version information available"。→ 铁律:补闭包必须同源。
- 产出:`poc/poc4-arch-isolation/`(build_closure.py + REPORT.md + 数据)。

### 新增文档

- `architecture/foundations/05-multi-source-and-isolation.md` —— **多源消费与隔离模型**。三类源(Nix省力/Arch/Debian补闭包)、补闭包同源铁律、分层隔离(默认轻env/loader、按需强namespace)、三种适配手段、多版本并存、与世代模型的张力处理。是 04-index-and-supply 的运行时下半段。

### 本轮设计共识

18. 不自造生态,消费 Nix(首选,自包含)/Arch/Debian(补闭包)。
19. 补闭包必须同源(ABI 自洽),绝不跨源拼库 —— PoC-4 实证的铁律。
20. 隔离分层:默认轻隔离(库视图),按需强隔离(namespace 沙箱)。
21. 隔离视图由世代派生,不破坏"世代=整机快照"语义。

### 待办

- PoC-4 只测简单 CLI 包;复杂包(dlopen/数据/setuid)补闭包难度待测。
- 同源补闭包需能拉取上游完整依赖树(Arch repo/nixpkgs/Debian)。
- 强隔离(namespace)、多包通信边界未做 PoC。

---

## 2026-06-09 —— 第五轮:PoC-2 二进制兼容实证

在真实 Linux(WSL Debian 13, glibc 2.41)上验证 Aevum 两大卖点之一"二进制开箱即跑"。

### PoC-2:二进制兼容(在真实 Linux 复现 NixOS 困境后验证)

- 方法:把 curl + 其 interpreter + 依赖库按内容寻址放进隔离 store;用 `unshare -rm` + tmpfs **遮蔽 `/lib64` 与 `/usr/lib/x86_64-linux-gnu`**,真实复现 NixOS"无标准 /lib"困境。
- 决定性结果:同一未修改二进制,**裸跑退出码 127(`cannot execute: required file not found`,即 NixOS 痛点),用 store 内 ld-linux + `--library-path` 启动退出码 0、正常输出 curl 版本**。
- 结论:"显式 loader 入口让普通二进制开箱即跑、无需 patchelf"在底层成立;剩下是做成默认透明的工程,非可行性问题。
- 产出:`poc/poc2-binary-compat/`(experiment.py + in_ns_test.py + REPORT.md)。

### 调整

- `architecture/runtime/03-binary-compat.md` §3 加 PoC-2 实证引用。

### PoC 矩阵现状(三个核心假设均已实证)

| PoC | 验证 | 结果 |
|---|---|---|
| PoC-1 | 包索引能否机器生成 | 仅 11.6% 纯自动 → 改为继承上游 |
| PoC-3 | 零 AI 能否求解 | 4 模板 442 包零未解析,可复现 |
| PoC-2 | 二进制能否开箱即跑 | 困境下裸跑死、Aevum loader 活 |

→ 三大风险(生态来源 / AI 依赖 / 二进制友好)全部有真实数据或可运行代码背书。

### 后续

- 走向实现:Rust workspace 代码骨架(store/generation/solver,可参照 PoC-3)。
- 可扩展:二进制开箱即跑率的更大样本测试、glibc 跨版本兼容。

---

## 2026-06-08(四)—— 第四轮:决策落地 + 补评审漏洞

用户拍板三个方向性决策(中心服务端 / Apache-2.0 许可 / 重心补设计漏洞),据此落地并系统性修复评审遗留的 HIGH 项。

### 决策落地

- **许可证定为 Apache-2.0**:新增 `LICENSE`(全文),README 更新 badge 与理由(企业友好 + 兼容继承上游元数据)。
- **走中心服务端路线**(类比 cache.nixos.org),但保留去中心降级。

### 新增文档

- `architecture/server/01-server-and-trust-root.md` —— **服务端架构 + 信任根**。中心服务端四服务(Index/Cache/Foundation Channel/Transparency Log)+ 信任根完整设计(Root→Channel 签名层级、出厂内置根、阈值签名、密钥轮换/撤销、透明日志、index_snapshot 锚定)。回应评审 **H1/H2**;明确"服务端宕机不影响已有 lock 运行"的中心化边界。
- `architecture/adr/0005-ai-model-form-factor.md` —— **AI 模型形态**。模型不进 Foundation、可插拔(本地/云/自带 key)、lock 记录 `ai_assist` 但重放不依赖、离线降级。回应评审 **H5**,基于 PoC-3。
- `architecture/runtime/07-host-coexistence.md` —— **宿主共存**。一切写在 $AEVUM_ROOT 内、只读引用宿主 /usr、PATH opt-in 投影、与宿主包管理器共存、干净卸载、per-user 免 root。回应评审 **H7**,补 ADR-0001 跳过的部分。

### 调整(回应 H4)

- `architecture/runtime/01-generation-lifecycle.md`:verify 从四类校验增至**五类**,新增"安全与版本回退判据"——CVE 命中/版本回退由 verify **机器独立判定**,不信任 AI 自述的 `needs_user_confirm`。切断"AI 既提议、又自评危险、还自我放行"的循环。
- `ai/01-maintainer-loop.md`:决策透明性补充说明 needs_user_confirm 不被单独信任。

### 本轮设计共识

14. 中心服务端提供真理来源与缓存,但客户端靠内容寻址+签名独立验真;服务端宕机不影响已有 lock。
15. 信任根:出厂内置 Root key,Root→Channel 签名链,Foundation/高敏感包阈值签名,撤销列表 + 透明日志防偷推。
16. AI 模型不属于 Foundation;三种部署可插拔;重放永不依赖模型。
17. 危险判定(CVE/回退)独立于提议者(AI),由 verify 机器强制。

### 评审遗留(后续)

- **H3 已修复**(本轮补):ADR-0004 新增 import 约束节 —— 沙箱 allowlist-only,仅 `@aevum/sdk` + 工程内相对路径,禁任意 npm/URL/动态 import,堵住配置期供应链面。至此评审 1 CRITICAL + 6 HIGH 全部处置完毕。
- 可继续 PoC-2:最小 System 层 + 主流二进制开箱即跑率(需 Linux 环境)。
- 走向实现:Rust workspace 代码骨架。

---

## 2026-06-08(三)—— 第三轮:Workflow 评审 + PoC-1/PoC-3 实证 + C1 修复

对全部设计做了一次 6 维度多代理 Workflow 评审(架构一致性/技术可行性/竞品差异/AI 风险/可用性/安全),并对高严重度发现做对抗式验证。评审定位头号风险 **C1:包索引来源缺失**。随后用 PoC-1/PoC-3 实测数据校准并修复。

### PoC-1:包索引可机器生成性实测

- 数据:整个 Debian stable(68,755 包、308,807 条依赖)+ 19 个真实 .deb 解 ELF。
- 结论:依赖元数据**仅 11.6% 能从二进制纯自动生成,41.5% 半自动,46.9% 是 ELF 永远不可见的人工语义**。自己从零造生态不可行。
- 出路:继承上游发行版人工策展 + ELF 校验补全 + AI 翻译。
- 产出:`poc/poc1-index-feasibility/`(REPORT.md + 可复现脚本 + 原始 JSON)。

### PoC-3:零 LLM 确定性求解器(回应 H5/H6)

- 纯 Python 确定性求解器,真实 Debian 数据,完全不调 LLM。
- 结论:4 个模板(dev-python/cli-tools/web-server/media,共 442 包)**零未解析**;同输入 closure_id 三次全等;lock 可不重新求解直接重放;6 项验证 all_pass。
- 意义:用代码兑现 ADR-0003 边界1 与 ADR-0004 可复现性主张,证明 **AI 是确定性骨架之上的可选增强,不是装软件的必需门槛**——H5/H6 的根被拔掉。
- 产出:`poc/poc3-zero-ai-solver/`(solver.py + verify.py + REPORT.md + lock 产物)。

### 新增文档

- `architecture/foundations/04-index-and-supply.md` —— **包索引与供给模型**。正面回答"生态从哪来":三层 bootstrap(导入上游元数据 → ELF 校验补全 → AI 翻译进能力模型 → 再签名)。把 C1 从"存亡级未知"降为"可工程化",基于 PoC-1 实测。

### 调整

- `comparison/01-nixos-pain-points.md`:§7 诚实声明扩写,新增 **§8 冷启动战略**(站在 Debian/nixpkgs 肩上,而非另起炉灶;附 PoC-1 数据)。
- `architecture/foundations/01-store.md`:`origin = "aevum-index:..."` 加注释,正式指向供给管线(此前被评审点名"被命名却未定义")。
- 索引补 foundations/04。

### 本轮新增设计共识

12. **Aevum 的创新在系统机制(可复现/原子/AI 维护),不在包生态**;生态继承上游,不原创。
13. 包索引 = 上游元数据导入 + ELF 自动校验 + AI 翻译 + 再签名 的产物,离线管线 + 人工复核,不在运行时热路径。

### 评审待办(后续可继续证伪/修复)

- H4 verify 增加 CVE/版本回退判据(防 AI 经"选签名旧版"绕过否决)。
- H5 AI 模型形态(本地/云、版本是否锁进 lock)—— **PoC-3 已缓解根因**:AI 不在求解/重放热路径,离线可用;剩余仅"自然语言翻译便利性"层面。
- H6 确认疲劳治理 —— **PoC-3 已缓解**:TOML 模板全流程零 AI,确认疲劳釜底抽薪。
- H3 TS 沙箱能否 import 任意包(配置期供应链面)。
- H7 宿主共存架构。
- 可继续 PoC-2:最小 System 层 + 主流二进制开箱即跑率实测。

---

## 2026-06-08(二)—— 第二轮:语言前端 + 借鉴竞品补强

基于对"是否需要像 NixOS 那样的语言"的讨论,以及对成熟工具(Pulumi/CDK、GitOps、Terraform、Snapper、OSTree、Nix cache)的借鉴,扩充设计。

### 新增决策

- **ADR-0004:意图层增加 TypeScript 可选第二前端**。在沙箱中求值(禁 IO/网络/时钟/随机),产出与 TOML 相同的 resolved/lock。**不推翻 ADR-0002,而是精炼它** —— 0002 否决的是"强制学小众烂语言",0004 增加的是"AI 极熟的主流语言作为可选、沙箱化前端"。TOML / 自然语言前端永久保留。

### 新增文档

**运行时机制** `architecture/runtime/`
- `04-state-vs-package-rollback.md` —— 状态 vs 包回滚。区分"包/配置回退"(已有机制)与"可变数据(数据库/用户文件)回退"(用 btrfs/zfs 子卷快照与世代绑定协调)。填补此前真空,是连 NixOS 都没解好的硬骨头。
- `05-generation-diff-plan.md` —— 世代 diff/plan。在 verify 与 activate 之间插入关卡:展示将要发生的变更 + AI 用人话解释 + 风险标注。对标 terraform plan / nix store diff-closures,但 AI 负责解读。
- `06-remote-cache.md` —— 远程缓存与增量传输。内容寻址二进制缓存(substituters)+ OSTree 式增量去重拉取 + 签名防投毒。

**AI 维护者** `ai/`
- `04-reconciliation-loop.md` —— 调和循环。把 Maintainer 从"命令响应器"升级为 GitOps 式"自治收敛器":期望态(active lock) vs 实际态持续比对,自愈漂移,改变期望态才问人。与"AI 是第一维护者"定位天然契合。

### 调整

- `architecture/adr/0004-*` 加入 ADR 列表。
- runtime 索引补 04/05/06;ai 索引补 04。
- 顶层 README 与 runtime/02 的"意图层"描述从"仅 TOML"更新为"TOML / 自然语言 / TS 三前端,共享 resolved/lock"。

### 本轮新增设计共识

7. 意图层三前端(TOML / 自然语言 / TS),TS 沙箱求值,可复现仍只来自 lock。
8. 包回滚与状态回滚是两回事,需协调;用户数据默认不随世代回退。
9. 激活前必须可 plan + AI 解释 + 风险标注;高风险变更强制确认。
10. Maintainer 是常驻调和循环:自愈朝期望态收敛,改变期望态走 plan + 问人。
11. 远程缓存是性能优化,不改可复现语义;索引映射必须签名。

### 待办(后续阶段)

- 开 Workflow 多代理对整个设计做一次整体评估(架构一致性、可行性、风险盲点)。
- `guides/` 构建与使用指南(待代码骨架落地)。
- `@aevum/sdk`(TS 意图 API)+ 沙箱求值器设计。
- 状态回滚事务性、二进制 interpreter 注入、块级去重等实现细节定稿。

---

## 2026-06-08(一)—— 设计文档体系初版成稿

项目立项,完成第一版完整设计文档体系。

### 立项决策(经用户确认)

- **项目名**:Aevum(拉丁语"世代/永恒",对应核心的 Generation 机制)。
- **定位**:对标 NixOS 的 AI-native 用户态系统层 —— 比 NixOS 更智能。建在 Linux 之上,**先桌面、兼顾服务器**。
- **技术栈**:Rust(待代码阶段)。
- **本阶段交付**:仅文档。

### 立项前调研(关键依据)

- 确认 NixOS 真实痛点:Nix 语言学习曲线、Flakes 撕裂、**普通二进制跑不起来**(最高频实操痛点)、依赖失败靠人肉、配置膨胀。详见 [`comparison/01-nixos-pain-points.md`](comparison/01-nixos-pain-points.md)。
- 确认竞品格局:**osModa**(AI-native OS,但建在 NixOS 上、保留 Nix 语言、主打服务器)、nixai(只是助手)、镜像级不可变发行版(无细粒度世代/无 AI)、Guix(仍是 DSL)、AIOS(给 agent 用的 OS,目标正交)。结论:Aevum 的精确位置无人占据,且 osModa 验证了方向的市场信号。详见 [`comparison/02-prior-art.md`](comparison/02-prior-art.md)。

### 新增文档

**架构主线** `architecture/`
- `00-overview.md` —— 架构总览(六核心对象、三文件层、端到端生命周期)
- `adr/0001-positioning-vs-nixos.md` —— 定位为用户态系统层
- `adr/0002-no-dsl-intent-layer.md` —— 意图层不引入图灵完备 DSL
- `adr/0003-ai-maintainer-authority.md` —— AI 维护者三条权限边界
- `foundations/01-store.md` —— 内容寻址存储
- `foundations/02-generation.md` —— 世代模型
- `foundations/03-closure.md` —— 依赖闭包与求解(AI/求解器职责分离)
- `runtime/01-generation-lifecycle.md` —— 世代状态机(propose/verify/activate/rollback)
- `runtime/02-intent-resolved-lock.md` —— 三文件层
- `runtime/03-binary-compat.md` —— 普通二进制兼容(正面回应 NixOS 痛点)

**分层隔离** `layers/`
- `README.md` —— Foundation/System/App 三层模型
- `01-foundation.md` —— 密封核心层
- `02-system-and-app.md` —— 系统层与软件层的共享/私有边界

**模板系统** `templates/`
- `README.md` —— 模板总览
- `01-template-model.md` —— 模板数据模型与合并语义

**AI 维护者** `ai/`
- `README.md` —— Maintainer 总览与三边界
- `01-maintainer-loop.md` —— 维护循环
- `02-repair-and-keep-two.md` —— 冲突修复与"保留两份"
- `03-garbage-collection.md` —— GC 引用统计与回收

**对比与竞品** `comparison/`
- `01-nixos-pain-points.md` —— NixOS 痛点剖析(带出处)
- `02-prior-art.md` —— 竞品与既有工作

**根**
- `README.md` —— 项目门面
- `docs/README.md` —— 文档总索引

### 核心设计共识(本版钉死)

1. 每次变更 = 新世代,旧世代永不修改(原子回滚)。
2. AI 是依赖链第一维护者,但锁在三边界笼子里:不直接选 hash、不动 Foundation、关键决策人类可否决。
3. Foundation/System/App 三层物理隔离,稳定层永不被软件层炸穿,foundation-only 兜底。
4. 实在解不了就保留两份(store 多版本并存),GC 在引用归零后归还磁盘。
5. 意图层无 DSL,逻辑收进模板 + AI + 求解器。
6. 二进制兼容是一等需求。

### 待办(后续阶段)

- `guides/` 构建与使用指南(待代码骨架落地)。
- Rust workspace 代码骨架。
- 运行时视图隔离("保留两份"的具体机制)、二进制 interpreter 注入等实现细节定稿。
