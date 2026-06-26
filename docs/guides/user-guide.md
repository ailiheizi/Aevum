# Aevum 使用教程

本文档面向想要实际使用 Aevum 的用户,从安装到日常使用全面覆盖。

---

## 目录

1. [安装与初始化](#1-安装与初始化)
2. [基本概念](#2-基本概念)
3. [从 Debian 源装包](#3-从-debian-源装包)
4. [从 Nix 源装包](#4-从-nix-源装包)
5. [TS 配置详解](#5-ts-配置详解)
6. [模板系统](#6-模板系统)
7. [世代管理](#7-世代管理)
8. [导出可运行系统](#8-导出可运行系统)
9. [配置漂移检测](#9-配置漂移检测)
10. [故障排除](#10-故障排除)

---

## 1. 安装与初始化

### 编译

```bash
# 需要 Rust 1.85+
cd Aevum
cargo build --release -p aevum-cli
cp target/release/aevum ~/.local/bin/  # 或任意 PATH 目录
```

### 初始化 AEVUM_ROOT

```bash
export AEVUM_ROOT=~/.aevum
mkdir -p $AEVUM_ROOT

# 拉 Debian 包索引(约 54MB,一次性)
bash scripts/prep-index.sh
# 产出:$AEVUM_ROOT/index/Packages

# 加 profile/bin 到 PATH(一次性,写进 .bashrc/.zshrc)
echo 'export AEVUM_ROOT=~/.aevum' >> ~/.bashrc
echo 'export PATH="$AEVUM_ROOT/profile/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### 验证

```bash
aevum --help
# 应输出命令列表
```

---

## 2. 基本概念

| 概念 | 含义 |
|------|------|
| **Store** | 内容寻址存储(`$AEVUM_ROOT/store/`),每个文件按 SHA256 hash 存,天然去重 |
| **Generation(世代)** | 一次系统状态的不可变快照(`$AEVUM_ROOT/generations/gen-NNN/`) |
| **Lock** | 确定性闭包锁定(`$AEVUM_ROOT/locks/*.lock`),保证可复现 |
| **Profile** | 当前活跃世代的可执行文件入口(`$AEVUM_ROOT/profile/bin/`) |
| **模板** | 声明式蓝图(`$AEVUM_ROOT/templates/*.toml`),可继承组合 |

### 数据流

```
TS 配置 / 自然语言 / 包名
       ↓
  确定性求解器(选版本、算闭包)
       ↓
  Lock 文件(可复现快照)
       ↓
  下载 .deb / NAR(SHA256 校验)
       ↓
  入 Store(内容寻址)
       ↓
  建世代(symlink 布局)
       ↓
  Verify 门禁(完整性/闭合/层)
       ↓
  激活(原子切换 active 指针)
       ↓
  Profile/bin 刷新(PATH 即用)
```

---

## 3. 从 Debian 源装包

### 最简(显式包名)

```bash
aevum maintain busybox-static ripgrep \
  --gen 1 --mirror http://mirrors.ustc.edu.cn/debian \
  --lock my-system --yes --confirm
```

### 用 TS 配置

```bash
cat > system.config.ts << 'EOF'
export default defineSystem(() => ({
  uses: ["ripgrep", "fd-find", "busybox-static"]
}));
EOF

aevum maintain --config system.config.ts \
  --gen 1 --mirror http://mirrors.ustc.edu.cn/debian \
  --lock system --yes --confirm
```

### 输出解释

```
[maintain] TS 配置: system.config.ts
  求值+模板展开产出 3 条约束:
    - busybox-static
    - fd-find
    - ripgrep
  → gen-1(从 TS 配置求解的 lock 起跑主循环)
  ① 求解: 闭包 7 个包 → locks/system.lock
  ② propose: 候选 gen-1 已造(343 个 store 对象,未激活)
  ③ verify: 硬性校验通过(完整性/闭合/层)
  ④ 激活: ✓ active 已切到 gen-1
```

### 使用

```bash
aevum switch 1
# profile/bin 自动刷新
rg --version        # ripgrep 14.1.1
busybox uname -a    # Linux ...
```

---

## 4. 从 Nix 源装包

Aevum 能直接消费 Nix 的 binary cache(无需安装 Nix):

### 按包名拉取

```bash
# --resolve 从 store-paths 查包名对应的 hash
# --activate 把 bin/ 链到 profile
aevum nix-fetch --resolve ripgrep --activate
aevum nix-fetch --resolve helix --activate
aevum nix-fetch --resolve niri --activate
```

### 按 hash 拉取

```bash
# 如果你已知包的 store path hash(32 字符)
aevum nix-fetch f4y36sn7m173qvdija8a1p6v81py66ns --activate
```

### 自定义镜像

```bash
# 默认用 USTC 镜像;可换清华/其他
aevum nix-fetch --resolve firefox \
  --mirror https://mirrors.tuna.tsinghua.edu.cn/nix-channels/store \
  --channel https://mirrors.tuna.tsinghua.edu.cn/nix-channels/nixpkgs-unstable
```

### Debian + Nix 混用

两个源安装的包通过同一个 `$AEVUM_ROOT/profile/bin` 暴露:

```bash
# Debian 源
aevum maintain --config system.config.ts --gen 1 --mirror ... --yes --confirm
aevum switch 1

# Nix 源(追加到同一个 profile/bin)
aevum nix-fetch --resolve helix --activate

# 同一个 PATH 里两个生态的包共存
rg --version    # Debian ripgrep
hx --version    # Nix helix
```

---

## 5. TS 配置详解

TS 配置在纯 Rust boa 引擎里沙箱求值(不需要 Node.js)。

### 沙箱规则

- ✅ 纯计算、条件、循环、类型注解
- ✅ `import` 仅 `@aevum/sdk` + 相对路径 `./`
- ❌ 文件 IO / 网络 / `Date.now()` / `Math.random()` / npm 包

### SDK API

```typescript
import { defineSystem, useTemplate } from "@aevum/sdk";

export default defineSystem((inputs) => {
  // inputs 是 --inputs JSON 传入的对象(记录进 lock,可复现)

  // useTemplate:选用蓝图模板
  const sys = useTemplate("dev-rust");

  // sys.use:直接带入包名
  sys.use("python3");

  // sys.override:钉版本
  sys.override("python3", { version: "3.11" });

  // sys.exclude:排除包
  sys.exclude("telemetry-agent");

  return sys;
});
```

### 显式输入(Inputs)

```bash
aevum maintain --config my.ts --inputs '{"role":"dev","gpu":true}' ...
```

inputs 被记录进 lock(`ts_inputs:` 行),确保可复现。审计时自动用记录的值重放。

### 声明式写法(不用 useTemplate)

```typescript
export default defineSystem(() => ({
  uses: ["ripgrep", "fd-find", "git"],
  overrides: { git: "2.40" },
  excludes: ["telemetry"]
}));
```

---

## 6. 模板系统

模板是 TOML 格式的声明式蓝图:

### 文件位置

```
$AEVUM_ROOT/templates/<name>.toml
```

### 格式

```toml
[template]
name = "dev-rust"
title = "Rust 开发环境"
version = "1.0.0"
extends = ["minimal-desktop"]  # 继承父模板

[capability.rustc]
constraint = ">=1.75"          # 版本约束
layer_hint = "app"             # 层建议(不可为 foundation)

[capability.cargo]
constraint = ">=1.75"
layer_hint = "app"

[optional.rust-analyzer]       # 可选组件
default = "true"               # 默认开启
```

### 继承与合并

- `extends`:深度优先展开,子覆盖父(同 id 约束)
- 多模板叠加:后声明覆盖先声明
- 优先级:用户 override > 子模板 > 父模板

### 无环校验

继承链有环(A→B→A)会报错,不死循环。

---

## 7. 世代管理

### 切换

```bash
aevum switch 2        # 切到 gen-2,profile/bin 自动刷新
aevum switch 1        # 回切(秒级)
```

### 回滚

```bash
aevum rollback 1      # 等价 switch 但语义更明确
```

### GC

```bash
aevum gc --keep 3     # 保留最近 3 个世代,其余删除
```

### 查看

```bash
ls $AEVUM_ROOT/generations/       # gen-001, gen-002, ...
cat $AEVUM_ROOT/generations/gen-001/lock.txt   # store 对象列表
readlink $AEVUM_ROOT/generations/active        # 当前活跃世代
```

---

## 8. 导出可运行系统

把世代导出为完整 rootfs(可 chroot/nspawn/QEMU 启动):

```bash
aevum export-system --generation 1 --out /tmp/my-system

# 进入(需 root)
sudo chroot /tmp/my-system /bin/sh
# 或
sudo systemd-nspawn -D /tmp/my-system
```

导出的 rootfs 包含:
- 世代全部文件(按 rel_path 铺平)
- `/etc/passwd`、`/etc/group`(root 用户)
- `/bin/sh` → busybox(如果世代含 busybox)
- `/proc`、`/sys`、`/dev`、`/tmp` 占位目录

### QEMU 引导

```bash
# 打包成 initramfs
cd /tmp/my-system && find . | cpio -o -H newc | gzip > /tmp/initramfs.cpio.gz

# 用 Debian 内核引导
qemu-system-x86_64 -kernel /path/to/vmlinuz \
  -initrd /tmp/initramfs.cpio.gz \
  -append "rdinit=/bin/sh console=ttyS0" \
  -nographic -m 512
```

---

## 9. 配置漂移检测

验证历史 lock 是否仍能由当前源配置重新产出:

```bash
# 先产 lock
aevum resolve --config my.ts --inputs '{"x":1}' --name my-lock --yes

# 之后验证(CI 里跑)
aevum audit-config my.ts --against my-lock
# 未漂移 → exit 0
# 漂移 → exit 1 + 报告差异
```

漂移原因:源 .ts 改了、模板改了、或包索引更新了。

---

## 10. 故障排除

### "未解析" 包

```
⚠ 未解析(前10): ["python3"]
```

原因:lock 里的版本约束(如 `=3.11`)在当前索引里精确匹配不到。
解法:放宽约束(`>=3.10`)或更新索引(`bash scripts/prep-index.sh`)。

### verify 完整性校验失败

```
完整性: xxx-libc.so.6 — 内容校验失败
```

原因:索引过期——索引记录的 SHA256 与镜像上当前 .deb 内容不一致(Debian 滚动更新)。
解法:重拉索引 `bash scripts/prep-index.sh`。

### nix-fetch "narinfo 拉取失败"

原因:该 store path 在 binary cache 里没有预编译(source-only 包)。
解法:换个包名/版本,或用 `--resolve` 重新查找。

### WSL `/tmp` 被清

WSL2 空闲时关闭实例会清 `/tmp`。解法:把 `CARGO_TARGET_DIR` 和 `AEVUM_ROOT` 放 `~/`(持久)。

### 代理(国内网络)

```bash
# WSL 内访问 GitHub/Nix cache 需代理
export http_proxy=http://127.0.0.1:7890
export https_proxy=http://127.0.0.1:7890
```

---

## 附:国内镜像推荐

| 用途 | 镜像 URL |
|------|----------|
| Debian 包 | `http://mirrors.ustc.edu.cn/debian` |
| Nix binary cache | `https://mirrors.ustc.edu.cn/nix-channels/store` |
| Nix channel | `https://mirrors.ustc.edu.cn/nix-channels/nixpkgs-unstable` |
| 备选 Nix cache | `https://mirrors.tuna.tsinghua.edu.cn/nix-channels/store` |
