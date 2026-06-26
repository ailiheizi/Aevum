# NixOS 痛点剖析 —— Aevum 到底在解决什么

> 父文档:[`../README.md`](../README.md)
> 关联:竞品 [`02-prior-art.md`](02-prior-art.md)、二进制兼容 [`../architecture/runtime/03-binary-compat.md`](../architecture/runtime/03-binary-compat.md)

---

## 0. 立场

> NixOS 的**理念是对的**(可复现、原子、回滚、声明式),Aevum 全盘继承。
> NixOS 的**实现方式劝退**(逼人写函数式 DSL、二进制生态摩擦),Aevum 全力替换。

下面是有据可查的具体痛点,每条都标注出处。这些就是 Aevum 设计的"问题清单"。

---

## 1. 必须学一门函数式 DSL(最大的墙)

NixOS 用 Nix 语言描述系统配置。Nix 是一门小众的、惰性求值的、纯函数式语言,几乎只为 Nix 生态存在。

**痛在哪**:

- 学习曲线陡峭,且这门语言的知识**几乎不能迁移**到别处(不像学 Python/Go 有通用价值)。
- 报错信息出了名地难读,惰性求值让错误现场和根因常常相距很远。
- 文档零散、过时、互相矛盾,初学者踩坑后很难自查。

来源:
- [The Curse of NixOS](https://blog.wesleyac.com/posts/the-curse-of-nixos) —— 用了三年 NixOS 的人对其根本性摩擦的剖析。
- [NixOS pain points — newbie-gone-intermediate experience report](https://discourse.nixos.org/t/nixos-pain-points-newbie-gone-intermediate-experience-report/452)
- [Smoothing the flakes learning curve](https://discourse.nixos.org/t/smoothing-the-flakes-learning-curve/11667) —— 复杂度、typo、过时信息叠加导致的学习困境。

**Aevum 的解法**:意图层是纯数据(TOML),不是图灵完备语言;逻辑/组合/求解交给模板系统 + AI 维护者。见 [`../architecture/adr/0002-no-dsl-intent-layer.md`](../architecture/adr/0002-no-dsl-intent-layer.md)。

---

## 2. Flakes 撕裂生态、加重心智

Flakes 是 NixOS 改善可复现的实验性特性,但长期处于"实验性却人人都用"的尴尬,新老两套写法并存。

**痛在哪**:

- 初学者要同时面对 channels 老路线和 flakes 新路线,概念叠加。
- flakes 强制的输入元组等设计对新人不友好。

来源:
- [NixOS and Flakes for beginners sucks!](https://discourse.nixos.org/t/nixos-and-flakes-for-beginners-sucks/62968)
- [My painpoints with flakes](https://discourse.nixos.org/t/my-painpoints-with-flakes/9750)
- [Outlining the differences between Flakes and Nix configs](https://discourse.nixos.org/t/outlining-the-differences-between-flakes-and-nix-configs/72996)

**Aevum 的解法**:单一意图模型,没有"稳定 vs 实验"两套割裂心智;可复现由 lock 层统一保证(见 [`../architecture/runtime/02-intent-resolved-lock.md`](../architecture/runtime/02-intent-resolved-lock.md))。

---

## 3. 普通二进制跑不起来(最高频实操痛点)

下载的预编译二进制在 NixOS 上几乎必然失败:

```text
Could not start dynamically linked executable: ./app
```

**痛在哪**:

- NixOS 没有标准 `/lib`、`/lib64`、`/usr/lib`;一切在 `/nix/store/<hash>`。
- 普通 ELF 的 interpreter 路径(`/lib64/ld-linux-x86-64.so.2`)找不到 → 直接拒绝运行。
- 解法要懂 `nix-ld`、`patchelf --set-interpreter`、FHS 环境 —— 全是底层折腾。

来源:
- [Packaging/Binaries — NixOS Wiki](https://nixos.wiki/wiki/Packaging/Binaries):"Downloading and attempting to run a binary on NixOS will almost never work."
- [Could not start dynamically linked executable (cargo run)](https://discourse.nixos.org/t/could-not-start-dynamically-linked-executable-on-cargo-run/58946)
- [Fixing 'Could not start dynamically linked executable' (uvx mcp server)](https://blog.kaorubb.org/en/posts/nixos-fix-could-not-start-dynamically-linked-executable/)
- [RPATH, or why lld doesn't work on NixOS](https://matklad.github.io/2022/03/14/rpath-or-why-lld-doesnt-work-on-nixos.html)

**Aevum 的解法**:把二进制兼容当**一等需求**,默认提供链接器入口 + 共享运行时基线,普通二进制开箱即跑。见 [`../architecture/runtime/03-binary-compat.md`](../architecture/runtime/03-binary-compat.md)。这是 Aevum 最硬的差异化之一。

---

## 4. 依赖/构建失败靠人肉排查

```text
error: 1 dependencies of derivation '/nix/store/....drv' failed to build
```

**痛在哪**:这类错误的根因排查高度依赖经验,新手往往卡死。

来源:
- [Error: 1 dependencies of derivation failed to build](https://discourse.nixos.org/t/error-1-dependencies-of-derivation-nix-store-drv-failed-to-build/39705)
- [What is the right way to debug linking in NixOS](https://discourse.nixos.org/t/what-is-the-right-way-to-debug-linking-in-nixos/16602)

**Aevum 的解法**:AI 维护者自动诊断冲突、提多个修复方案、各自验证、择优;实在不行保留两份。见 [`../ai/02-repair-and-keep-two.md`](../ai/02-repair-and-keep-two.md)。

---

## 5. 配置膨胀后的维护负担

随着系统复杂,Nix 配置膨胀成大型代码库,组织、复用、重构都需要更高的 Nix 功力(module 系统、overlay、lib 函数)。

来源:
- [My Experience of NixOS](https://thiscute.world/en/posts/my-experience-of-nixos/)
- [Organizing your Nix configuration without flakes](https://somas.is/note-organizing-nix-configuration-without-flakes.html):"layer on top of an already complex language to learn."

**Aevum 的解法**:模板系统用声明式蓝图做组合/派生/覆盖,不需要写代码;团队标准环境固化成可分享的 TOML 模板。见 [`../templates/`](../templates/)。

---

## 6. 痛点 → 解法对照总表

| NixOS 痛点 | 出处 | Aevum 解法 | 设计文档 |
|---|---|---|---|
| 必须学 Nix 语言 | curse-of-nixos | 意图层纯数据,无 DSL | ADR-0002 |
| Flakes 撕裂生态 | flakes-for-beginners | 单一意图模型 | runtime/02 |
| 二进制跑不起来 | Packaging/Binaries | 默认二进制兼容 | runtime/03 |
| 依赖失败靠人肉 | derivation-failed | AI 诊断+多方案+保留两份 | ai/02 |
| 配置膨胀难维护 | my-experience-of-nixos | 声明式模板组合 | templates/ |

---

## 7. 诚实声明:Aevum 也有代价

不回避取舍:

- **依赖 AI**:首次求解/修复需要 AI 在场(但历史世代重放纯靠 lock,不需要 AI)。
- **内容寻址 + 保留两份占盘**:由 GC 按引用回收缓解(见 [`../ai/03-garbage-collection.md`](../ai/03-garbage-collection.md))。
- **二进制兼容不可能 100%**:覆盖绝大多数普通二进制,极端依赖仍需专门处理。
- **生态不是原创的,是借来的**:见下方 §8 的冷启动战略。

这些代价记录在案,设计上尽量用机制对冲,但不假装它们不存在。

---

## 8. 冷启动战略:站在巨人肩上,而非另起炉灶

> 这是 Aevum 最大的现实风险,也是经 PoC-1 实测数据校准后的正式立场。

### 8.1 现实:NixOS 真正的护城河是 nixpkgs,不是 Nix 语言

我们花大力气拆掉了"Nix 语言"这堵墙,但必须诚实:NixOS 十年积累的真正资产是 **nixpkgs**(庞大的、人工策展的包集与依赖元数据)。如果 Aevum 要从零攒一套等价生态,那是另一个十年人力黑洞,定位根本不成立。

### 8.2 PoC-1 实测:依赖元数据无法靠机器从零生成

用整个 Debian stable(68,755 包、308,807 条依赖)+ 真实 ELF 抽样实测(详见 [`PoC-1 报告`](../../poc/poc1-index-feasibility/REPORT.md)):

| 依赖元数据 | 占比 | 能否机器自动生成 |
|---|---|---|
| 纯库依赖(ELF `DT_NEEDED` 可出) | **11.6%** | 🟢 全自动 |
| 库依赖 + 版本约束 | **41.5%** | 🟡 半自动 |
| 非库 / 虚包 / alternatives | **46.9%** | 🔴 ELF 永远看不见,纯人工语义 |

**结论:近半依赖是机器无从得知的人工语义(`nginx-core` 的 14 条依赖里 ELF 能给出 0 条)。自己从二进制生成完整索引 = 不可行。**

### 8.3 战略:继承 + 增强,而非重造

Aevum 的生态战略明确为三层(技术实现见 [`../architecture/foundations/04-index-and-supply.md`](../architecture/foundations/04-index-and-supply.md)):

```text
1. 继承:复用上游发行版(Debian/Arch/nixpkgs)的人工策展元数据
         → 那 46.9% 的人工语义,继承而非自造
2. 增强:ELF 自动分析做校验补全(11.6%+41.5%),AI 做元数据翻译
3. 分阶段铺开:初期只覆盖一个上游(如 Debian)子集,逐步扩大
```

### 8.4 这不丢人,也不削弱定位

- Ubuntu 借 Debian、无数发行版借上游,都是站在巨人肩上 —— 这是发行版的常态,不是耻辱。
- **Aevum 的创新从来不在"又攒了一套包",而在系统机制**:内容寻址世代、原子回滚、AI 维护依赖链、分层隔离、二进制友好、无强制 DSL。这些才是它相对 NixOS 的真差异化(§1–§5)。
- 把"生态借来"摆上台面,反而让定位更可信:我们清楚自己的创新边界在哪。

---

## 参考来源

- [The Curse of NixOS](https://blog.wesleyac.com/posts/the-curse-of-nixos)
- [NixOS pain points (discourse)](https://discourse.nixos.org/t/nixos-pain-points-newbie-gone-intermediate-experience-report/452)
- [NixOS and Flakes for beginners sucks!](https://discourse.nixos.org/t/nixos-and-flakes-for-beginners-sucks/62968)
- [Packaging/Binaries (NixOS Wiki)](https://nixos.wiki/wiki/Packaging/Binaries)
- [RPATH, or why lld doesn't work on NixOS](https://matklad.github.io/2022/03/14/rpath-or-why-lld-doesnt-work-on-nixos.html)
- [My Experience of NixOS](https://thiscute.world/en/posts/my-experience-of-nixos/)
