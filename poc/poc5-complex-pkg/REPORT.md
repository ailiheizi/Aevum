# PoC-5:复杂包补闭包压测 — 实测报告

> 日期:2026-06-09 · 状态:已完成 · **找到 PoC-4 算法的架构盲区**
> 环境:WSL Debian 13 + 真实 Arch 包(python 3.14.5、imagemagick 7.1.2.25)
> 目的:回答"复杂包(dlopen 插件/运行时数据/深依赖)会不会崩",压测 PoC-4 的"递归解 NEEDED 补闭包"算法。
> 关联:[`PoC-4`](../poc4-arch-isolation/REPORT.md)(简单包补闭包)、[`foundations/05`](../../docs/architecture/foundations/05-multi-source-and-isolation.md)

---

## 0. 为什么做这个 PoC

PoC-4 用 ripgrep 证明了"补闭包 + 轻隔离"可行,但 ripgrep 是**理想样本**:依赖少、无插件、无运行时数据。当时报告诚实标注:"复杂包(dlopen/数据/setuid)难度更高,未测"。

本 PoC 专挑会踩坑的复杂包,验证 PoC-4 的核心算法(**递归解主二进制的 `DT_NEEDED` 来补闭包**)对它们是否成立。**结论:不成立,且找到了确切的盲区。** 这正是 PoC 的价值——在写代码前发现算法缺陷。

---

## 1. 复杂包的隐藏结构(实测数据)

| 包 | 主二进制 NEEDED | 隐藏依赖(NEEDED 看不见) |
|---|---|---|
| **python 3.14** | 仅 `libpython3.14.so.1.0`, `libc.so.6` | • 写死路径的标准库 `/usr/lib/python3.14/`(.py)<br>• **77 个** `lib-dynload/*.so`(import 时 dlopen)<br>• 这些 .so **各自还有 NEEDED**(如 `_ssl.so` → `libssl.so.3` + `libcrypto.so.3`) |
| **imagemagick** | (主二进制少量) | **137 个**编解码器插件 `.so`(`modules-Q16HDRI/coders/`,运行时 dlopen) |

---

## 2. 决定性发现:DT_NEEDED 补闭包对复杂包是不完整的

```text
python3.14 主二进制 DT_NEEDED = { libpython3.14.so.1.0, libc.so.6 }
   ↓ PoC-4 算法:递归解这两个就以为闭包完整了
   ↓ 但运行时真相:
       import ssl  → dlopen lib-dynload/_ssl.so
                   → _ssl.so 又 NEEDED libssl.so.3 + libcrypto.so.3
       这整条链,主二进制的 NEEDED 里【完全看不到】
   ↓ 后果:
       python 能启动(主二进制闭包够),但 `import ssl` 当场崩
       imagemagick 能启动,但打开多数图片格式(137 插件)失败
```

**架构裁决**:

> **"递归解主二进制 DT_NEEDED"对简单包(ripgrep)够用,对复杂包(python/imagemagick)漏掉整片运行时 dlopen 依赖。复杂包会崩,且崩在原算法的盲区里。**

NEEDED 是**链接期**依赖;复杂包大量用**运行时** dlopen(按路径加载插件),这是两套机制,后者 ELF 静态分析根本看不见。

---

## 3. 这不是死局:补闭包算法的架构修正

复杂包的隐藏依赖虽然 NEEDED 看不见,但**有迹可循**,可以补进算法:

```text
完整补闭包 = DT_NEEDED 递归(链接期)
           + 包元数据声明的运行时目录(.PKGINFO/nixpkgs 都标了标准库、插件路径)
           + 对插件目录内每个 .so 也递归解 NEEDED(深一层闭包)
           + 写死的数据路径(标准库 .py、配置)整目录纳入
```

具体三条修正:

1. **不止扫主二进制,扫整个包的所有 ELF**:python 的 77 个 lib-dynload、imagemagick 的 137 个 coders,逐个解 NEEDED,把它们的依赖(libssl/libcrypto…)也纳入闭包。
2. **运行时数据路径整目录纳入**:`/usr/lib/python3.14/` 这种写死路径的标准库目录,整体进 store(不只是 .so,还有 .py)。
3. **保留包内部布局**:dlopen 按相对/绝对路径找插件,补闭包后这些路径要在隔离视图里能解析到(这也呼应了下面 §4 的符号链接问题)。

→ 本质:**补闭包不能只信 ELF 静态分析,要结合"包元数据声明的运行时结构"**。这恰好印证了 [`foundations/04`](../../docs/architecture/foundations/04-index-and-supply.md) "继承上游元数据"的必要性——上游早就标好了标准库目录和插件路径,Aevum 该用它,而不是纯靠 ELF 反推。

---

## 4. 附带发现:符号链接是 store 的一等公民

解包时(Windows NTFS 侧)大量失败:

```
python→python3→python3.14(链)
imagemagick: animate/compare/convert/... 全是 magick 的符号链接
libpython3.14.so → libpython3.14.so.1.0
```

复杂包**大量用符号链接**组织(版本别名、多命令共享一个二进制)。架构含义:

> **Aevum 的内容寻址 store 必须把符号链接当一等公民正确保留**(不能解引用成副本,否则 137 个 magick 软链变成 137 份拷贝,且破坏包预期的布局)。

这是 [`foundations/01-store`](../../docs/architecture/foundations/01-store.md) 内容规范化时必须处理的:符号链接本身是内容的一部分。

---

## 5. 结论

| 问题 | 裁决 |
|---|---|
| 复杂包会不会崩? | **会**,如果用 PoC-4 的"只递归主二进制 NEEDED"算法 |
| 崩在哪? | 运行时 dlopen 的插件/扩展及其依赖(NEEDED 盲区) |
| 是死局吗? | **不是**。算法可修正:扫全包 ELF + 纳入元数据声明的运行时目录 + 整目录纳入数据路径 |
| 修正代价? | 比简单包高,但有上游元数据指路(印证 04 的"继承元数据") |
| 额外发现 | store 必须正确保留符号链接(复杂包大量用) |

**总判断**:复杂包暴露了 PoC-4 算法的不完整,但没有暴露"做不出来"——它指向一个明确的、可工程化的算法修正。**这正是该在写 Rust 前发现的事**:如果直接照 PoC-4 实现,python/imagemagick 这类包会在用户面前崩;现在知道了,实现时补闭包逻辑就该从一开始包含 dlopen/数据路径/元数据三条。

---

## 6. 对设计文档的影响

- [`foundations/05`](../../docs/architecture/foundations/05-multi-source-and-isolation.md) 的"补闭包"需补充:**复杂包必须扫全包 ELF + 纳入元数据声明的运行时目录/插件路径**,不能只递归主二进制 NEEDED。
- [`foundations/01-store`](../../docs/architecture/foundations/01-store.md) 需明确:**符号链接是内容的一部分,规范化时保留不解引用**。
- 强化 [`foundations/04`](../../docs/architecture/foundations/04-index-and-supply.md):运行时结构(标准库目录、插件路径)只能从上游元数据获得,ELF 静态分析不够 —— 这是"必须继承上游"的又一硬证据。

---

## 7. 复现方式

```bash
# 下载(Windows 侧有网+zstd)
curl -o py.pkg.tar.zst https://geo.mirror.pkgbuild.com/core/os/x86_64/python-3.14.5-1-x86_64.pkg.tar.zst
curl -o im.pkg.tar.zst https://geo.mirror.pkgbuild.com/extra/os/x86_64/imagemagick-7.1.2.25-1-x86_64.pkg.tar.zst
# 在 Linux 侧解包(NTFS 会因符号链接失败 —— 这本身是发现之一)
# 分析:主二进制 NEEDED vs 运行时 dlopen 目录(lib-dynload / ImageMagick coders)
# 取证 _ssl.so 的 NEEDED → libssl/libcrypto(主二进制看不见的深层闭包)
```

---

## 8. 一句话总结

> **python(77 dlopen 扩展)和 imagemagick(137 插件)证明:只递归主二进制 DT_NEEDED 的补闭包算法对复杂包不完整,会漏掉运行时 dlopen 的整片依赖,import ssl 即崩。
> 但这不是死局——补闭包须"扫全包 ELF + 纳入上游元数据声明的运行时目录/插件路径",且 store 须保留符号链接。复杂包没否定方案,而是精确校正了实现算法,且再次证明"必须继承上游元数据"。**
