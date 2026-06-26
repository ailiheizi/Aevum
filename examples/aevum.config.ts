// aevum.config.ts —— TS 意图前端示例(ADR-0004)
//
// 这是意图层的「可编程前端」:在沙箱里求值,产出与 intent.toml 等价的声明式意图,
// 再走同一套确定性求解器 → 同一 lock。可复现只来自 lock,与用哪个前端无关。
//
// 用法:
//   aevum resolve --config aevum.config.ts
//   aevum resolve --config aevum.config.ts --inputs '{"role":"developer"}'
//
// 沙箱红线(求值期物理禁止,违反即报错):
//   ✗ 文件 IO / 网络 / Date.now() / Math.random() / 隐式读环境变量
//   ✗ import 任意 npm 包 / URL import / 动态 import()
//   ✓ 纯计算、条件、循环、类型检查;import 仅 @aevum/sdk + 工程内相对路径
//
// 需要「按环境变化」的合法需求,通过显式 inputs 传入(被记录进 lock → 仍可复现),
// 而不是偷偷读环境。

import { defineSystem, useTemplate } from "@aevum/sdk";

export default defineSystem((inputs) => {
  // 基础蓝图(模板):useTemplate 选用模板,CLI 用 template crate 展开成一组能力约束
  // (含继承的父模板与 default 开启的可选组件);模板只给约束不给 hash。
  const sys = useTemplate("minimal-desktop");

  // 可编程组合:按显式输入条件启用
  if (inputs.role === "developer") {
    sys.use("python3");
    sys.use("git");
  }

  // 循环生成:为清单里每个工具带入(纯计算,确定性)
  for (const tool of inputs.tools ?? []) {
    sys.use(tool);
  }

  // 声明式覆盖:钉版本(求解器据此精确求 hash)
  sys.override("python3", { version: "3.11" });

  // 排除:从意图中剔除某包
  sys.exclude("telemetry-agent");

  return sys;
});
