# Aevum

<div align="center">

**AI-native、可复现、原子化的 Linux 用户态包管理器**

**消费 Debian + Nix 两大生态的预编译包,用 TypeScript 声明意图,一键装系统**

[![Status](https://img.shields.io/badge/status-functional--prototype-green.svg)](docs/README.md)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

[English](README.md) · 简体中文

</div>

---

## 快速开始

```bash
# 1. 初始化(一次性):建好 profile + 索引
export AEVUM_ROOT=~/.aevum
aevum init --update                # 建目录骨架 + 拉 Debian 索引
source $AEVUM_ROOT/profile/env.sh  # 加到 .bashrc

# 2. 装包(像 apt 一样简单)
aevum install ripgrep foot busybox-static

# 3. 直接用!
rg --version    # ripgrep 14.1.1
foot --version  # Wayland 终端 1.21.0
busybox ls /    # busybox 内置命令

# 搜索 / 列出 / 移除
aevum search wayland   # 搜索可装的包
aevum list             # 列出当前装了什么
aevum remove foot      # 移除(建新世代,可回滚)
aevum rollback 1       # 秒回上一个状态
```

或者从 **Nix binary cache** 拉任意 nixpkgs 的包:

```bash
# 从 Nix 镜像拉 niri(Wayland tiling compositor)+ 全部 242 个依赖
aevum nix-fetch --resolve niri --activate

# 直接可用(通过同一个 profile/bin PATH)
niri --version  # niri 26.04 (Nixpkgs)
```

或者用 **TS 配置** 声明复杂系统:

```bash
cat > my-system.config.ts << 'EOF'
export default defineSystem(() => ({
  uses: ["weston", "foot", "busybox-static", "ripgrep"]
}));
EOF

aevum maintain --config my-system.config.ts --gen 1 \
  --mirror http://mirrors.ustc.edu.cn/debian --yes --confirm
```

---

## 这是什么

Aevum 是一个用 Rust 实现的 Linux 用户态包管理器,核心理念:

- **不自建包生态**:消费 Debian 镜像(.deb)和 Nix binary cache(NAR)两大现有生态的预编译包
- **TypeScript 声明意图**:用主流语言(不是 Nix 语言)在沙箱里声明系统配置,确定性求解
- **内容寻址 store**:每个文件按 SHA256 hash 存储,天然去重、可复现
- **原子世代切换**:每次变更是一个不可变的 generation,一键回滚
- **AI 增强可选**:AI 翻译自然语言意图、自动修复冲突,但 AI 是可选增强不是门槛

---

## 核心命令

| 命令 | 功能 |
|------|------|
| `aevum init [--update]` | 初始化 root(profile + 目录 + env.sh);`--update` 顺带拉索引 |
| `aevum ai "<自然语言>"` | **AI 统一入口**:自动判断意图(装包/解释/搜索...),多轮对话 |
| `aevum install <pkg...>` | 快捷安装(自动求解+下载+建世代+激活+刷新 PATH) |
| `aevum search <keyword>` | 搜索可安装的包 |
| `aevum list` | 列出当前世代的包 |
| `aevum remove <pkg...>` | 移除包(建新世代) |
| `aevum update` | 更新 Debian 包索引 |
| `aevum maintain --config <ts>` | 从 TS 配置全链路:求解→下载→入库→建世代→verify→激活 |
| `aevum resolve --config <ts>` | 只求解产 lock(不下载安装) |
| `aevum switch <gen>` | 切换世代(原子,自动刷新 profile) |
| `aevum rollback <gen>` | 回滚到历史世代 |
| `aevum nix-fetch --resolve <name>` | 从 Nix cache 拉包+依赖 |
| `aevum nix-fetch <hash> --activate` | 拉包并链到 profile/bin |
| `aevum audit-config <ts> --against <lock>` | 检测配置是否漂移 |
| `aevum export-system <gen>` | 导出可运行 rootfs(chroot/nspawn) |
| `aevum gc --keep <gen-id,...>` | 垃圾回收(保留指定世代 id 引用的对象) |
| `aevum explain <message>` | AI 解释错误/给建议 |

> CLI 还有更多进阶命令(`verify`、`activate`、`build`、`compose-generation`、`export-bootroot`、`boot-menu`、`service`、`etc`)。跑 `aevum --help` 看完整列表。

---

## 安装

### 前置条件

- Linux(原生或 WSL2)
- Rust 1.85+(编译用)
- `curl`、`ar`、`tar`、`xz`(运行时,下载解包用)

### 从源码编译

```bash
git clone https://github.com/ailiheizi/Aevum
cd Aevum
cargo build --release -p aevum-cli
# 二进制在 target/release/aevum
```

> Windows 上请在 WSL2 里编译——NTFS 不支持 Aevum 依赖的 symlink。

> **离线编译**:默认从 crates.io 拉依赖。要离线/气隙编译,先联网跑一次 `cargo vendor vendor`,再 `cp .cargo/config.offline.toml .cargo/config.toml`。生效的 `.cargo/config.toml` 已被 gitignore,绝不破坏 clean clone 和 CI。

### 初始化

```bash
export AEVUM_ROOT=~/.aevum  # 或任意目录
mkdir -p $AEVUM_ROOT

# 拉 Debian 包索引(一次性)
aevum update

# 确保 PATH 含 profile/bin
echo 'export PATH="$AEVUM_ROOT/profile/bin:$PATH"' >> ~/.bashrc
```

---

## 使用教程

### 1. TS 配置前端

Aevum 用 TypeScript 声明系统意图(在纯 Rust boa 沙箱里求值,不需要 Node.js):

```typescript
// aevum.config.ts
import { defineSystem, useTemplate } from "@aevum/sdk";

export default defineSystem((inputs) => {
  // 选用模板(蓝图,展开成一组包约束)
  const sys = useTemplate("minimal-desktop");

  // 按输入条件启用
  if (inputs.role === "developer") {
    sys.use("python3");
    sys.use("git");
  }

  // 循环
  for (const tool of inputs.tools ?? []) {
    sys.use(tool);
  }

  // 钉版本
  sys.override("python3", { version: "3.11" });

  // 排除
  sys.exclude("telemetry-agent");

  return sys;
});
```

运行:
```bash
aevum maintain --config aevum.config.ts \
  --inputs '{"role":"developer","tools":["ripgrep"]}' \
  --gen 1 --mirror http://mirrors.ustc.edu.cn/debian --yes --confirm
```

TS 沙箱禁 IO/网络/时钟/随机,import 走 allowlist——配置求值保持确定性(ADR-0004)。

### 2. 模板系统

模板是声明式蓝图(`templates/<name>.toml`),声明"想要什么能力":

```toml
# templates/dev-rust.toml
[template]
name = "dev-rust"
version = "1.0.0"
extends = ["minimal-desktop"]  # 继承

[capability.rustc]
constraint = ">=1.75"
layer_hint = "app"

[capability.cargo]
constraint = ">=1.75"
layer_hint = "app"

[optional.rust-analyzer]
default = "true"
```

模板支持继承(extends)、无环校验、optional 开关、override 覆盖。

### 3. Nix 包源

从 Nix binary cache 拉取任意 nixpkgs 包(无需安装 Nix):

```bash
# 按包名查找 + 递归拉依赖 + 链到 PATH
aevum nix-fetch --resolve ripgrep --activate
aevum nix-fetch --resolve niri --activate
aevum nix-fetch --resolve helix --activate

# 或直接指定 store hash
aevum nix-fetch f4y36sn7m173qvdija8a1p6v81py66ns --activate

# 自定义镜像
aevum nix-fetch --resolve firefox \
  --mirror https://mirrors.tuna.tsinghua.edu.cn/nix-channels/store \
  --channel https://mirrors.tuna.tsinghua.edu.cn/nix-channels/nixpkgs-unstable
```

### 4. 世代管理

```bash
# 查看世代
ls $AEVUM_ROOT/generations/

# 切换(原子,自动刷新 profile/bin)
aevum switch 2

# 回滚
aevum rollback 1

# 垃圾回收(保留世代 1、2、3 引用的对象)
aevum gc --keep 1,2,3
```

### 5. 导出可运行系统

```bash
# 导出 rootfs(可直接 chroot/nspawn/QEMU)
aevum export-system 1 --out /tmp/my-rootfs

# 进入
sudo systemd-nspawn -D /tmp/my-rootfs
# 或
sudo chroot /tmp/my-rootfs /bin/sh
```

### 6. 配置漂移检测

```bash
# 检测源配置是否与 lock 一致(CI 友好,漂移时返回非零)
aevum audit-config my-system.config.ts --against my-lock
```

---

## AI 功能

Aevum 的 AI 是**可选增强**(无 AI 时确定性核心照常工作)。你只需记**一个命令** `aevum ai`。

### 配置(一次性)

编辑 `$AEVUM_ROOT/config.toml`:

```toml
[ai]
provider = "deepseek"   # deepseek / openai / claude / ollama
api_key = "sk-..."      # 或设环境变量 AEVUM_AI_KEY
```

| Provider | Endpoint | Key 环境变量 |
|----------|----------|-------------|
| deepseek | api.deepseek.com | `DEEPSEEK_API_KEY` |
| openai | api.openai.com | `OPENAI_API_KEY` |
| claude | api.anthropic.com | `ANTHROPIC_API_KEY` |
| ollama | localhost:11434 | 无需(本地) |

### `aevum ai` —— 一个命令,自然语言,自动判断意图

```bash
aevum ai "我要个 python 数据科学环境"
# 💬 我帮你装 python3 + numpy + pandas + jupyter
# → 意图: 安装 → 确认? → 装

aevum ai "再加上 git"            # 多轮:读历史,理解"再加上"
aevum ai "为什么 numpy 装不上"   # 自动判断 → 解释
aevum ai "libfoo 和 libbar 冲突咋办"  # 自动判断 → 分析依赖冲突
aevum ai "列出装了什么"          # 自动判断 → 列包
aevum ai --reset                 # 清空对话历史,开新话题
```

AI 自己判断意图(install / explain / repair / search / list / gc / chat),
分发到对应动作。**有副作用的动作(装包/卸载)默认要确认**,只读的直接执行。
对话历史存盘(`ai-history.txt`),支持多轮接续。

### AI 修依赖冲突

出现版本冲突时,确定性求解器先算出可行的修复方案(A 放宽 / B 升父包 / C 保留两份 / D 告知用户),
AI 再**选风险最低的方案并解释理由**——跟着求解器的可行解走,绝不自己编版本:

```
⚠ 检出 1 处版本冲突:
    libfoo 已选 1.0,但 app-q 要求 (= 2.0) — 未满足
    ↳ 方案A 不适用: libfoo 无单一版本同时满足 ["= 1.0", "= 2.0"]
    ↳ 方案C(需确认): libfoo 保留两份 — 1.0 给 app-p,2.0 给 app-q

  🤖 AI 分析冲突中(deepseek/deepseek-chat)...
  AI 推荐方案 C: 保留 libfoo 1.0 和 2.0 两份版本
  理由: libfoo 无法通过放宽约束共存,保留两份安全且不影响其他依赖。
  (方案 C 需人工确认,不自动执行)
```

### AI 的边界(ADR-0003/0005)

- AI 只在 **lock 之前**介入(判断意图、翻译包名、评估冲突修复)
- lock 之后的 propose/verify/activate **全程无 AI**——可复现只来自 lock
- AI 不可用时,确定性核心(install/求解/世代)照常工作;意图翻译降级到离线 Mock

> 底层命令(`maintain --intent`、`explain`、`install` 等)仍可直接用,但日常推荐 `aevum ai`。

---

## 架构

```
┌─────────────────────────────────────────────────────────┐
│  意图层(TS 前端 / TOML / 自然语言 / 模板)             │
├─────────────────────────────────────────────────────────┤
│  确定性求解器(6.8 万包索引,可复现)                    │
├─────────────────────────────────────────────────────────┤
│  内容寻址 Store(SHA256 + 去重)                         │
├────────────────────────┬────────────────────────────────┤
│  Debian .deb 源        │  Nix binary cache 源           │
├────────────────────────┴────────────────────────────────┤
│  世代管理(原子切换 / 回滚 / GC / verify 门禁)         │
├─────────────────────────────────────────────────────────┤
│  Profile/bin(统一 PATH 入口)                           │
└─────────────────────────────────────────────────────────┘
```

### Crate 结构

| Crate | 功能 |
|-------|------|
| `cli` | 命令行入口 + 编排逻辑 |
| `solver` | 确定性闭包求解器 |
| `store` | 内容寻址对象存储 |
| `generation` | 世代管理(创建/切换/回滚/GC) |
| `config-ts` | TS 前端(boa 沙箱求值) |
| `template` | 模板系统(继承/合并/展开) |
| `nix-source` | Nix binary cache 客户端(NAR 解包) |
| `intent` | AI 意图翻译层 |
| `closure-builder` | ELF 运行闭包构建 |
| `maintainer` | verify 门禁(完整性/闭合/层) |
| `service-compiler` | s6 服务编译 |
| `etc-builder` | /etc 配置编译 |
| `elf` | ELF 解析(DT_NEEDED) |

---

## 与 NixOS 的关系

Aevum **不是 NixOS 的替代品**,而是走了一条不同的路:

| | NixOS | Aevum |
|---|---|---|
| 包生态 | 自建(nixpkgs 8万+) | 消费 Debian + Nix 两家 |
| 配置语言 | Nix(小众 DSL) | TypeScript(主流,沙箱化) |
| 构建系统 | derivation(从源码) | 直接消费预编译包 |
| AI 角色 | 无(nixai 是外挂) | 内置可选增强(翻译意图/修冲突) |
| 复杂度 | 极高(学 Nix 语言门槛) | 低(写 TS 或选模板) |
| 可复现 | 来自 Nix 语言求值 | 来自 lock(与前端无关) |

Aevum 可以**消费 Nix 的产出**(`nix-fetch` 直接拉 nixpkgs 预编译包)而不需要用户学 Nix 语言。

---

## 已验证能力

| 场景 | 验证结果 |
|------|----------|
| 静态链接程序(busybox) | ✅ 直接可用 |
| 动态链接程序(ripgrep) | ✅ 自动补全依赖闭包 |
| Wayland 终端(foot) | ✅ 35 包闭包,WSLg 上运行 |
| Wayland compositor(weston) | ✅ 250 包 GPU 栈,WSLg 弹窗 |
| Nix 包(niri) | ✅ 242 包递归拉取,--version 成功 |
| QEMU 引导 | ✅ 内核 → Aevum initramfs → shell |
| 世代切换 | ✅ switch 后 PATH 自动可用 |
| 配置漂移检测 | ✅ 同源未漂移 / 改源报漂移 |
| AI 修依赖冲突 | ✅ 4 场景实测,方案 A/C/D 都选对 |

---

## 文档

- [架构总览](docs/architecture/00-overview.md)
- [模板系统](docs/templates/README.md)
- [Nix 包源设计](docs/design/nix-source.md)
- [变更日志](docs/CHANGELOG.md)(58 轮迭代记录)
- [ADR](docs/architecture/adr/)(5 个架构决策记录)
- [PoC](poc/)(7 个概念验证)

---

## 许可证

Apache-2.0
