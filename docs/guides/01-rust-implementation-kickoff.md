# Rust 实现启动指南

> 从设计走向实现的第一份操作指南。前提:已读 [`../../OVERVIEW.md`](../../OVERVIEW.md) 和 [`../../CLAUDE.md`](../../CLAUDE.md)。
> 目标:把 7 个 PoC 校正过的算法,落成第一版能 `cargo build` 的 Rust 代码。

---

## 0. 心法

PoC 已经证明"机制可行 + 抓出了实现陷阱"。这一步不是重新设计,是**把已验证的算法翻译成 Rust**,并从第一行就避开 PoC 抓到的坑。每个 crate 都有对应的 PoC 作参照实现。

不要追求一次到位。目标顺序:**能 build → 能装一个简单包(rg)→ 能多版本 → 能回滚 → 能 GC → 复杂包(python)**。

---

## 1. Workspace 结构(建议)

```
aevum/
├── Cargo.toml                 # workspace
├── crates/
│   ├── store/                 # 内容寻址存储
│   ├── generation/            # 世代:创建/原子切换/回滚
│   ├── solver/                # 确定性闭包求解
│   ├── closure-builder/       # 补闭包(ELF 分析 + 元数据)
│   ├── elf/                   # ELF 解析(NEEDED/interp/RPATH)——被 closure-builder 用
│   └── cli/                   # aevum 命令行入口(后期)
```

---

## 2. 四个核心 crate,各自落地哪个 PoC

### `elf`(先做,被多处依赖)
- 解析 ELF:PT_INTERP、DT_NEEDED、DT_RPATH/RUNPATH。
- 参照:PoC-2/4/5 的 Python 解析器(`poc/poc*/`,纯 struct 解析,逻辑可直译)。
- 建议用现成 crate `goblin` 或 `object` 而非手写(PoC 手写是为了零依赖验证,生产用库)。

### `store`(内容寻址)
- 内容 → sha256 → `store/<hash>-<name>/`,不可变、加载期校验。
- **PoC-6 铁律**:语义权限位(可执行/setuid/setgid/sticky)纳入哈希输入;入库/取出显式恢复权限位(`std::os::unix::fs::PermissionsExt`),否则 sudo 提权失效。
- **PoC-5 铁律**:符号链接保留(`symlink_metadata` / 创建 symlink),不解引用。
- 规范化:固定排序 + 清零 mtime,但**保留权限语义**。
- 参照设计:[`../architecture/foundations/01-store.md`](../architecture/foundations/01-store.md)。

### `generation`(世代)
- 世代目录 = layers/{foundation,system,app}/ 下 symlink 到 store hash + lock 文件。
- **PoC-7 已验证机制**:active 是 symlink;切换 = 写临时 symlink + `std::fs::rename`(原子);回滚 = 指针回指,不重建。
- 参照实现:`poc/poc7-core-mechanics/experiment.py`(set_active / make_generation 直译)。
- 参照设计:[`../architecture/foundations/02-generation.md`](../architecture/foundations/02-generation.md)、[`../architecture/runtime/01-generation-lifecycle.md`](../architecture/runtime/01-generation-lifecycle.md)。

### `solver`(确定性求解)
- 输入约束集 → 传递闭包 → 精确 hash 集 + closure_id。
- **PoC-3 已是完整参照**:`poc/poc3-zero-ai-solver/solver.py`(280 行,含 Debian 版本比较、版本选择、传递展开、alternatives 确定性处理、closure_id)。**几乎可逐函数直译成 Rust。**
- 铁律:确定性(固定排序、无随机/时钟);AI 不参与这一层(ADR-0003)。

### `closure-builder`(补闭包)
- 给定一个包,产出它的完整运行闭包。
- **PoC-5 校正后的算法(必须四源合一)**:
  1. 主二进制 DT_NEEDED 递归
  2. 扫**全包所有 ELF**(插件/扩展),各自解 NEEDED 纳入其依赖
  3. 上游元数据声明的运行时目录(标准库 / 插件路径)整体纳入
  4. 写死的数据路径整目录纳入
- **PoC-4 铁律**:同源补闭包(库从包自己那一源取,不跨源)。
- 参照:`poc/poc4-arch-isolation/build_closure.py`(递归补闭包骨架)+ PoC-5 报告的算法修正。

---

## 3. 第一个里程碑:装一个 rg 并回滚

最小闭环,验证四个 crate 串起来能用:

```
1. closure-builder: 解 Arch/Debian rg → 补闭包(rg + libc + libgcc_s + libpcre2)
2. store: 闭包内每个文件内容寻址入库(权限位/symlink 正确处理)
3. generation: 造 gen-1(链接到这些 hash)+ 设 active
4. 跑通 rg --version(轻隔离:显式 loader + library-path,见 PoC-2)
5. 造 gen-2(换个 rg 版本)→ 原子切 active → 回滚到 gen-1
6. gc: 删 gen-2 后回收无引用 hash,共享库不误删(见 PoC-7)
```

这个闭环跑通 = Aevum 核心成立。复杂包(python,PoC-5 的 dlopen 坑)留到第二里程碑。

---

## 4. 测试策略

- 每个 crate 的单测直接复用 PoC 的验证用例(PoC-3 的 closure_id 三次一致、PoC-7 的 GC 不误删、PoC-6 的 setuid 往返)。
- 集成测试在 WSL/真 Linux 跑(symlink/ELF/权限),CI 同理。
- 用真实包做 fixture(Arch rg 已在 `poc/poc4-arch-isolation/data/`)。

---

## 5. 不要做的事

- 不要在 store 里跨源拼库(ABI 崩,PoC-4)。
- 不要只递归主二进制 NEEDED 就以为闭包完整(复杂包崩,PoC-5)。
- 不要让 AI 直接决定装哪个 hash(破坏可复现,ADR-0003)。
- 不要把 LLM 权重塞进 Foundation(ADR-0005)。
- 不要在 Windows 侧解包测试(NTFS symlink 失败)。

---

## 6. 同步维护

- 实现中发现设计需调整 → 更新对应 docs + [`../CHANGELOG.md`](../CHANGELOG.md)。
- 本指南完成后,补充真正的"构建/使用指南"到本 guides 目录。
