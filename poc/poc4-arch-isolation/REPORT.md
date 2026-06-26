# PoC-4:Arch 包"补闭包 + 轻隔离"验证 — 实测报告

> 日期:2026-06-09 · 状态:已完成 · 核心机制证实 + 一个关键风险被暴露
> 环境:WSL Debian 13 (trixie), glibc 2.41, x86_64 —— 真实 Linux
> 目的:验证"用隔离方式消费多源包(如 Arch)"是否可行,回答两个 PoC 级未知。
> 关联:[`PoC-2`](../poc2-binary-compat/REPORT.md)(二进制兼容机制)、生态战略讨论。

---

## 0. 为什么做这个 PoC

讨论"Aevum 能不能用 Nix/Arch 等多源的包、能不能隔离式多包并存"时,出现两个**只靠推理无法回答、必须实测**的疑问:

- **疑问1(轻隔离扛不扛多源)**:Arch 包不像 Nix 自包含,它靠标准路径 `/usr/lib` 找库。给它一个"私有库视图"(轻隔离),它在没有标准 `/lib` 的环境真能跑吗?
- **疑问2(补闭包成本)**:把一个非自包含的 Arch 包"补全成闭包"(凑齐它要的所有库),工作量到底多大?

这正是当初三个 PoC 帮项目避开的那种"纸上通、一做就炸"的风险点,所以实测。

---

## 1. 实验对象与方法

- 对象:真实 Arch 包 `ripgrep-15.1.0-3-x86_64.pkg.tar.zst`(从 Arch 官方镜像 geo.mirror.pkgbuild.com 下载、zstd+tar 解开)。
- 方法:
  1. 解析 Arch `rg` 二进制的 ELF,拿到 interpreter + NEEDED。
  2. **递归补闭包**:从 rg 出发解 NEEDED → 取库 → 再解库的 NEEDED,直到闭合,每个按内容寻址放进隔离 store。
  3. 用 `unshare -rm` + tmpfs **遮蔽 `/lib64` 和 `/usr/lib`**,复现"无标准库"环境。
  4. 对比:**A 裸跑** vs **B Aevum 轻隔离**(store 内 loader + 私有 `--library-path`)。

---

## 2. 关键事实:Arch 包是"非自包含"的(和 Debian 同构)

```
Arch rg 二进制:
  interpreter: /lib64/ld-linux-x86-64.so.2     ← 标准路径(不是自己的)
  NEEDED:      libpcre2-8.so.0, libgcc_s.so.1, libc.so.6   ← 只写库名,无路径
  RPATH/RUNPATH: 无                              ← 不写死路径,靠系统找
.PKGINFO 依赖声明: glibc, libgcc, pcre2          ← 包名(粗粒度)
```

对比维度:

| | Nix 包 | Arch 包(本 PoC) | Debian 包(PoC-2) |
|---|---|---|---|
| 找库方式 | RUNPATH 写死自包含闭包 | 标准路径 `/usr/lib` | 标准路径 `/lib` |
| 自包含 | ✅ 是 | ❌ 否 | ❌ 否 |
| 多版本并存 | 天然 | 否(需适配) | 否(需适配) |

→ **Arch 和 Debian 同属"靠标准路径"阵营,不自包含。要在 Aevum 里多版本并存,必须补闭包 + 隔离。** Nix 则天生就是闭包。

---

## 3. 结果

### 3.1 补闭包(回答疑问2)

```
闭包内容: rg + ld-linux + libc.so.6 + libgcc_s.so.1 + libpcre2-8.so.0
递归补全: closure_complete = true, libs_missing = 0
成本信号: 4 个库全部自动解析,0 个需人工
```

store 布局(内容寻址):
```
5de86430-rg/  0de72615-ld-linux-x86-64.so.2/  06e87bc6-libc.so.6/
30c61ab0-libgcc_s.so.1/  8ee551e7-libpcre2-8.so.0/
```

→ **简单 CLI 包(rg)的补闭包是全自动、零缺失的,成本低。** (注:复杂包——带 dlopen 插件、运行时数据、配置文件——会更难,本 PoC 未覆盖,见 §5。)

### 3.2 隔离跑(回答疑问1)—— 决定性对比

遮蔽标准库后,**同一个 Arch rg 二进制**:

```
[env] std loader exists? NO          ← 标准库已遮蔽,Arch 包失去依赖

A. 裸跑:    A_naive_rc=127
   stderr: rg: cannot execute: required file not found   ← 非自包含,挂

B. Aevum 轻隔离: B_isolated_rc=0
   stdout: ripgrep 15.1.0                                 ← 跑通!
```

→ **一个为 Arch 编译、从没打算在别处跑的二进制,被补成闭包 + 轻隔离后,在无标准库的环境里成功运行。轻隔离机制扛得住非自包含的多源包。**

---

## 4. 最有价值的发现:真正的坑不在"路径",在"ABI 版本兼容"

B 虽然 rc=0,但 stderr 有一条**必须正视**的警告:

```
libpcre2-8.so.0: no version information available (required by rg)
```

原因:本实验给 Arch 的 rg 喂的是**宿主 Debian 的 libc/pcre2**(实验环境所限)。同名库找到了、也跑起来了,但**符号版本不完全匹配**。ripgrep 这次没崩,是因为它用到的符号恰好兼容;换一个对 glibc 符号版本更敏感的程序,跨源喂库**会真崩**(`symbol not found`,不是 warning 而是致命错)。

### 这把"多源混跑会冲突"的担忧精确化了

```
错误的理解:多源包放一起,路径会冲突
实测的真相:隔离能解决路径冲突(已证);
           真正的坑是 ABI —— 给 Arch 包喂 Debian 的库,同名 ≠ ABI 兼容
```

### 而这恰恰强化了"隔离"方案的正确做法

> **每个包要带【它自己那一源】的完整闭包**(Arch 包带 Arch 的 glibc/pcre2,不跨源拼库),隔离开。
> 这样既无路径冲突(隔离解决),也无 ABI 串味(同源闭包解决)。

这正是 Nix 闭包和 Flatpak runtime 的做法,也印证了 Aevum store "内容寻址 + 每个包绑定自己的精确依赖 hash" 的设计是对的——**关键是补闭包时要从同源取库,而非从宿主随便凑**。

---

## 5. 诚实声明(本 PoC 没证的)

- **跨源 ABI 兼容未隔离验证**:本实验用宿主 Debian 库代替 Arch 库,所以 §4 的警告是"用错了源"的直接后果。真正的 Aevum 应从 Arch 源取 Arch 的库——这需要能拉取 Arch 完整依赖包,本 PoC 只拉了 rg 本体。
- **只测了简单 CLI 包**:rg 是静态感强、依赖少的理想案例。带 dlopen 插件(如 imagemagick)、运行时数据路径(如 nginx 的配置)、setuid 的包,补闭包难度会显著上升,未测。
- **轻隔离是手动拼 library-path**:真正"开箱即跑"需自动化(自动构造每个程序的私有视图),本 PoC 只证机制,未证体验。
- **没测多包同时跑 + 通信**:疑问1 里"隔离的包之间能否通信"未实测,只证了"单个 Arch 包隔离跑通"。

---

## 6. 结论:隔离式多源消费可行,但有纪律

| 维度 | 裁决 |
|---|---|
| Arch 包能否被 Aevum 隔离式消费 | ✅ 能(机制实测通过) |
| 补闭包成本(简单包) | 低(全自动零缺失) |
| 多版本/多源并存 | ✅ 隔离 + 内容寻址 store 天然支持 |
| 真正的风险 | **ABI 兼容**:必须从同源补闭包,不可跨源拼库 |
| 隔离方案是否正确 | ✅ 正确,且 ABI 发现强化了它——每包带同源完整闭包 |

→ **"做一个隔离多包的方式"这个直觉是对的,且实测可行。纪律是:每个包带它自己那一源的完整闭包,靠内容寻址 store 去重 + 轻隔离视图划界。** 这统一了 Nix(自带闭包)、Arch/Debian(补闭包)两类来源,都获得多版本并存 + 隔离无冲突。

---

## 7. 复现方式

```bash
# Windows 侧(有网+zstd):下载解包 Arch ripgrep
curl -o rg.pkg.tar.zst https://geo.mirror.pkgbuild.com/extra/os/x86_64/ripgrep-15.1.0-3-x86_64.pkg.tar.zst
zstd -d rg.pkg.tar.zst -o rg.pkg.tar && mkdir rg && tar -xf rg.pkg.tar -C rg
# WSL/Linux 侧:补闭包 + 遮蔽标准库对比
python3 build_closure.py                    # 递归补闭包 → /tmp/aevum-poc4/store
source /tmp/aevum-poc4/paths.sh
unshare -rm bash -c '
  mount -t tmpfs none /lib64; mount -t tmpfs none /usr/lib/x86_64-linux-gnu
  "$RG" --version; echo A_rc=$?                                   # 预期 127
  "$LD" --library-path "$LIBS" "$RG" --version; echo B_rc=$?      # 预期 0
'
```

---

## 8. 一句话总结

> **真实 Arch ripgrep 二进制,补闭包(全自动零缺失)+ 轻隔离后,在无标准库环境裸跑挂(rc127)、Aevum 隔离跑活(rc0,输出 ripgrep 15.1.0)。
> 隔离式多源消费机制可行;最大发现是真正的坑不在路径而在 ABI——必须从同源补闭包,这反而强化了"每包带同源完整闭包 + 内容寻址去重 + 轻隔离划界"这条路。**
