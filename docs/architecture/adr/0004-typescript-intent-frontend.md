# ADR-0004:意图层增加 TypeScript 可选第二前端(沙箱求值)

> 状态:**已接受(Accepted)** · 日期:2026-06-08
> 父文档:[`../00-overview.md`](../00-overview.md)
> 关联:[`0002-no-dsl-intent-layer.md`](0002-no-dsl-intent-layer.md)、[`../runtime/02-intent-resolved-lock.md`](../runtime/02-intent-resolved-lock.md)、[`../../comparison/01-nixos-pain-points.md`](../../comparison/01-nixos-pain-points.md)

> **实现进度(第四十九/五十轮,2026-06-19)**:最小可验证链路已落地。`crates/config-ts` 用 boa(纯 Rust JS 引擎)沙箱求值 `aevum.config.ts` → 约束 → 复用确定性求解器 → lock。一致性测试证明同语义 TOML 与 TS 前端产出**相同 closure_id**(本 ADR 核心红线兑现)。沙箱已禁随机/时钟、import allowlist 已堵第三方包(H3)。显式 inputs 已记入 lock 头部审计区(`ts_inputs:` 行,不进 closure_id,第五十轮)。**尚未**做:模板展开、**重放消费记录的 inputs**(目前只写不用)、inputs 进摘要以检测篡改、真抢占式求值超时、多文件相对 import 加载、SDK 签名分发、接入 maintain、恢复离线 vendor。详见 [`../../CHANGELOG.md`](../../CHANGELOG.md) 第四十九/五十轮"边界与待办"。

---

## 背景

ADR-0002 决定意图层不引入图灵完备 DSL,理由是 NixOS 的痛 = ① 图灵完备带来的非确定性/惰性求值地狱 + ② Nix 语言小众、报错烂、知识不可迁移。当时的结论是用纯数据(TOML)+ 模板 + AI 规避这两条。

但实践中出现两类 TOML 表达不了的真实需求:

- **可编程组合**:按主机名派生端口、按环境(dev/prod)条件启用组件、循环生成 N 个相似实例、从外部清单计算依赖集。
- **AI 的天然介质**:Maintainer 本就是 AI;让 AI 用它最擅长、有类型检查、有 LSP 的主流语言来表达复杂意图,比让它拼 TOML 更可靠、更可审。

关键洞察:**ADR-0002 真正要否决的是 ②(逼人类学小众烂语言),不是"语言"本身。** 如果语言是 AI 极熟的主流语言(TypeScript)、且确定性问题用沙箱解决,② 整个消失,只剩 ① 待处理 —— 而 ① 可以用"语言只负责写意图、不负责锁定"来化解。

---

## 决策

**意图层增加 TypeScript 作为可选的第二前端。TS 配置程序在纯沙箱中求值,产出与 TOML 前端完全相同的 `resolved` / `lock` 工件。TOML 仍是简单场景的默认前端。**

```
意图层的三种写法,都收敛到同一个 resolved/lock：
  ├─ 自然语言        →（AI 翻译）        → intent → resolved → lock
  ├─ intent.toml     →（直接解析）       → resolved → lock      ← 默认,大多数用户
  └─ aevum.config.ts →（沙箱求值 synth）→ intent → resolved → lock   ← 复杂/可编程场景
```

- 三个前端是平级的输入面,**lock 层不变**:可复现性仍然只来自 lock(内容寻址 + 索引快照 + 闭包),与用哪个前端无关。
- TS 前端是**可选**的:不写 TS 的用户完全不受影响,TOML / 自然语言照常工作。

---

## 这不是推翻 ADR-0002,是精炼它

| ADR-0002 要守的东西 | 本决策如何守住 |
|---|---|
| 用户不被逼学小众烂语言 | TS 是主流语言,AI 极熟;且**仍可选**,简单场景用 TOML/自然语言,没人被强制写 TS |
| 报错可读、知识可迁移 | TS 有类型系统、LSP、人话报错;TS 知识到处通用(对比 Nix 的 eval 期报错 + 知识不可迁移) |
| 可复现不依赖语言 | 可复现来自 lock,不来自语言;TS 程序求值后被钉成 lock,重放只用 lock |
| 配置不能有不可控副作用 | **沙箱强制**:求值期禁文件 IO、禁网络、禁时钟、禁随机(见下) |

> ADR-0002 的精神是"别让 NixOS 的语言之痛重演"。TS + 沙箱 + 可选,正好满足这个精神,同时补上 TOML 表达力不足的缺口。**两个 ADR 不冲突:0002 否决"图灵完备 DSL 作为唯一且强制的人类接口",0004 增加"主流语言作为沙箱化、可选的第二接口"。**

---

## 沙箱:化解确定性(①)的关键

TS 是图灵完备的,但 Aevum 让它只在"写意图"阶段跑,且关进沙箱:

```
aevum.config.ts
   ↓ 在沙箱中 evaluate（synth 阶段）
   沙箱强制：
     ✗ 文件系统访问        （禁,否则结果依赖机器状态 → 不可复现）
     ✗ 网络                （禁,否则结果依赖远端 → 不可复现）
     ✗ 时钟 Date.now()      （禁,否则结果依赖时间 → 不可复现）
     ✗ 随机 Math.random()   （禁,否则结果不确定）
     ✗ 环境变量隐式读取      （禁;需要的输入必须经显式、被记录的 inputs 传入）
     ✓ 纯计算、条件、循环、类型检查
   ↓ 输出
   一个确定的 intent 对象 → 照常 resolved → lock
```

**核心契约**:给定相同的 TS 源 + 相同的显式输入,沙箱求值**必然产出相同的 intent**。副作用被物理禁止,所以图灵完备不再威胁可复现。这是 Pulumi / AWS CDK 验证过的模式(真实语言 → synthesize 成声明式工件 → 部署/复现工件,而非语言)。

### 显式输入(替代隐式环境依赖)

需要"按主机名/环境变化"的合法需求,通过**显式声明的输入**满足,而非偷偷读环境:

```typescript
// 输入被声明、被记录进 lock,所以重放时输入也固定 → 仍可复现
export default defineSystem((inputs) => {
  const port = inputs.env === "prod" ? 443 : 8080;
  return {
    template: "server-web",
    overrides: { "nginx.port": port },
  };
});
```

`inputs` 的值在求值时确定并记录进 resolved/lock。重放历史世代时用记录的 inputs,不重新读环境 → 逐字节一致。

---

## TS 配置示例

```typescript
// aevum.config.ts
import { defineSystem, useTemplate } from "@aevum/sdk";

export default defineSystem((inputs) => {
  const sys = useTemplate("minimal-desktop");

  // 可编程组合:条件启用
  if (inputs.role === "developer") {
    sys.use("dev-rust");
    sys.use("dev-python-ds");
  }

  // 循环生成:为每个项目派生一个隔离的工具集
  for (const proj of inputs.projects ?? []) {
    sys.use("dev-web", { scope: proj });
  }

  // 声明式覆盖(类型检查保证字段合法)
  sys.override("python", { version: "3.11" });
  sys.exclude("telemetry-agent");

  return sys;
});
```

- 这是"写意图",不是"执行安装"。`sys` 最终被序列化成与 intent.toml 等价的声明式对象。
- `@aevum/sdk` 提供的 API 只能产出意图,不能触发副作用(沙箱再加一层强制)。
- TS 的类型系统当场拦截"version 写成数字""模板名拼错"这类错误 —— 正是 Nix 最烂的 eval 期才报错的反面。

---

## import 约束:堵住配置期供应链面(回应评审 H3)

评审 H3 指出一个真实风险:如果 `aevum.config.ts` 能 `import` 任意 npm 包,那么"运行期禁网络"挡不住"求值前已经把恶意依赖拉进来"——配置期就成了 RCE/供应链投毒入口。沙箱禁的是副作用,但一个恶意的 transitive import 可以在**求值过程中**通过纯计算(无 IO 也能)污染产出的意图,或利用沙箱逃逸漏洞。

因此沙箱在 import 层面追加硬约束:

```text
allowlist-only import,禁止任意第三方包：
  ✓ import 仅允许:
     - @aevum/sdk           （官方意图 API,随客户端分发、签名校验）
     - 同一配置工程内的相对路径 ./ ../（用户自己的 .ts 拆分文件）
  ✗ 禁止:
     - 任意 npm 包名 import（无 node_modules 解析）
     - URL import / 动态 import() 远程
     - 任何触发包获取的解析路径
```

具体保证:

1. **无包管理器解析**:沙箱的模块解析器**不挂 node_modules**,裸包名 import 直接解析失败 —— 从机制上根除"拉一个 npm 包进来"。
2. **@aevum/sdk 是被签名的可信内置**:它随客户端分发,走 Foundation/索引同源的签名校验(见 [`../server/01-server-and-trust-root.md`](../server/01-server-and-trust-root.md)),不是从公共 registry 拉的。
3. **相对 import 限定在配置工程内**:允许用户把大配置拆成多个 `.ts`,但路径不能逃逸出配置目录(防 `../../../etc` 式读取宿主文件——也与沙箱禁 IO 一致)。
4. **动态 import / URL import 一律禁**:消除"求值期临时拉代码"的可能。
5. **求值超时 + 资源上限**:即便全是本地纯计算,也设超时与内存上限,防恶意配置 DoS 求值器(图灵完备的固有风险)。

> 这把 TS 前端的供应链面收敛到"只信任一个被签名的官方 SDK + 用户自己工程内的文件"。复杂逻辑用户照样能写(条件、循环、拆文件),但**借第三方包夹带代码这条路被堵死**。代价是用户不能在配置里复用 npm 生态——这是有意的:意图配置不该依赖外部代码,需要的能力由 `@aevum/sdk` 显式提供。

---

## 为什么选 TypeScript(而非 Starlark / CUE / Dhall)

| 候选 | AI 熟练度 | 确定性来源 | 取舍 |
|---|---|---|---|
| **TypeScript + 沙箱** ✅ | **最高** | 沙箱强制(禁副作用) | 要设计沙箱;但最契合"前端/AI 熟悉"的初衷 |
| Starlark | 中 | 语言天生禁副作用 | 确定性更"免费",但 AI 没 TS 溜,生态小 |
| CUE / Dhall | 中低 | 天生确定 + 保证终止 | 小众,违背"AI 熟悉的语言"的核心诉求 |

选 TS 是因为本决策的出发点就是"用 AI 最熟的主流语言"。确定性靠沙箱补,这是可工程化的(Deno 的权限模型默认即禁 FS/网络,接近现成)。Starlark 作为"想让确定性由语言本身保证、不依赖沙箱纪律"时的备选记录在案。

---

## 代价与应对

| 代价 | 应对 |
|---|---|
| 沙箱实现有工程成本 | 复用成熟运行时的权限模型(如 Deno);沙箱是一次性投入,收益是整个可编程意图层 |
| 两个前端要保持语义等价 | 都收敛到同一个 intent 中间表示 + 同一套 resolved/lock;加一致性测试(同语义 TOML 与 TS 产出相同 lock) |
| 用户可能滥用 TS 写复杂逻辑 | 不强制;文档引导"简单用 TOML,复杂才上 TS";复杂度是用户自选的,不是被强加的 |
| 图灵完备 = 可能不终止 | 沙箱设求值超时;真正需要"保证终止"时可后续评估 Starlark 路线 |

---

## 边界(本决策的红线)

1. TS 前端**只在 synth 阶段、沙箱内**运行,产出 intent,**绝不**参与 activate/运行时。
2. 沙箱**必须**禁 IO/网络/时钟/随机/隐式环境读取;违反即视为 bug。
3. 沙箱**必须** allowlist-only import:仅 `@aevum/sdk`(签名内置)+ 配置工程内相对路径;**禁**裸 npm 包名 / URL import / 动态远程 import / node_modules 解析(回应 H3)。求值设超时与资源上限。
3. 可复现性的唯一来源仍是 **lock**,不是语言;任何"靠重跑 TS 才能复现"的设计都违背本决策。
4. TS 前端仍受 ADR-0003 三边界约束:它产出意图,不直接选 hash、不能触碰 Foundation。
5. TOML / 自然语言前端**永远保留**,TS 是增项不是替代(否则就违背了 ADR-0002 的"不强制学语言")。

---

## 影响

- [`../runtime/02-intent-resolved-lock.md`](../runtime/02-intent-resolved-lock.md) 的"意图层"从"仅 TOML"扩展为"TOML / 自然语言 / TS 三前端,共享 resolved/lock"。
- 需要一个 `@aevum/sdk`(TS)定义意图 API,以及一个沙箱求值器(synth runtime)。
- 一致性测试:等价的 TOML 与 TS 配置必须产出相同 closure_id。
- 本 ADR 与 ADR-0002 并存且互补;阅读 0002 时应一并参照本篇。
