//! AI 增强层:意图 → 约束适配器(见 `docs/ai/05-intent-resolver-implementation.md`)。
//!
//! 把模糊意图翻译成确定性求解器([`aevum_solver`])能吃的 `Vec<Constraint>`。
//! **不碰任何确定性逻辑**——AI 的非确定性被隔离在"产约束"阶段(ADR-0003 边界1)。
//!
//! 三种实现:
//! - [`MockIntentResolver`]:规则映射,离线、确定性、可测(不调网络/模型)。
//! - [`DeepSeekResolver`]:经系统 `curl` 调 DeepSeek(不引 Rust HTTP/JSON 依赖,沿用 zstd/gunzip 套路)。
//!
//! 统一 AI 客户端([`ai_client`]):支持 DeepSeek/OpenAI/Claude/Ollama,从 config.toml 配置。
//!
//! ADR-0005:确定性核心离线永久可用;AI 增强可选,模型不可用则 Err → 调用方降级到显式约束。

pub mod ai_client;

use aevum_solver::version::VerOp;
use aevum_solver::Constraint;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IntentError {
    #[error("AI 模型不可用,无法翻译自然语言意图: {0}(可降级到显式约束/模板)")]
    ModelUnavailable(String),
    #[error("意图翻译失败: {0}")]
    TranslateFailed(String),
}

/// 用户意图的三种形态(只有 NaturalLanguage 需要 AI)。
#[derive(Debug, Clone)]
pub enum Intent {
    /// 自然语言:"我要数据科学环境"(需 AI 翻译)。
    NaturalLanguage(String),
    /// 模板名(确定性,读模板→约束;此处占位,模板 crate 未实现)。
    Template(String),
    /// 直接约束(用户已写好,透传,无需 AI)。
    Explicit(Vec<Constraint>),
}

/// 意图翻译结果:约束集 + 可审计记录。
#[derive(Debug, Clone)]
pub struct IntentOutcome {
    /// 翻译出的约束集——喂给 `aevum_solver::resolve`。
    pub constraints: Vec<Constraint>,
    /// 可审计记录(写进 lock 的 ai_assist,ADR-0003 边界3 / ADR-0005)。
    pub assist: AiAssist,
}

/// AI 参与的可审计记录(lock 记录用,但重放不依赖它,ADR-0005)。
#[derive(Debug, Clone)]
pub struct AiAssist {
    /// 本次是否有 AI 介入(Template/Explicit=false,NaturalLanguage=true)。
    pub ai_involved: bool,
    /// 模型标识(如 "deepseek-chat" / "mock" / "none")。
    pub model_id: String,
    /// AI 产出的约束理由(供审计:为什么是这些约束)。
    pub reason: String,
}

/// 意图解析器(AI 增强层核心抽象,ADR-0003 边界1)。
pub trait IntentResolver {
    fn resolve_intent(&self, intent: &Intent) -> Result<IntentOutcome, IntentError>;
}

/// 把一行 `name`、`name>=ver`、`name=ver` 解析为 Constraint。
///
/// 模型输出与 Mock 规则都用这个统一格式 → 解析逻辑共享,无 JSON 依赖。
pub fn parse_constraint_line(line: &str) -> Option<Constraint> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // 找运算符
    for (sym, op) in [(">=", VerOp::Ge), ("<=", VerOp::Le), ("=", VerOp::Eq)] {
        if let Some(pos) = line.find(sym) {
            let name = line[..pos].trim();
            let ver = line[pos + sym.len()..].trim();
            if name.is_empty() || ver.is_empty() {
                continue;
            }
            return Some(Constraint {
                name: name.to_string(),
                op: Some(op),
                ver: Some(ver.to_string()),
            });
        }
    }
    // 无约束:纯包名(校验合法字符,避免把句子当包名)。
    if line
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'))
    {
        Some(Constraint::unconstrained(line))
    } else {
        None
    }
}

/// 把多行文本解析为约束集(模型响应/Mock 输出共用)。
pub fn parse_constraints(text: &str) -> Vec<Constraint> {
    text.lines().filter_map(parse_constraint_line).collect()
}

// ───────────────────────── MockIntentResolver ─────────────────────────

/// 确定性 mock:关键词 → 约束集映射(离线、可测,不调模型)。
pub struct MockIntentResolver {
    rules: Vec<(String, Vec<Constraint>)>,
}

impl MockIntentResolver {
    /// 内置几条常见意图规则(演示/测试用)。
    pub fn with_defaults() -> Self {
        let rules = vec![
            (
                "数据科学".to_string(),
                parse_constraints("python3\nnumpy\npandas"),
            ),
            ("python".to_string(), parse_constraints("python3")),
            ("基础工具".to_string(), parse_constraints("coreutils\ngrep\nsed")),
        ];
        MockIntentResolver { rules }
    }

    pub fn with_rules(rules: Vec<(String, Vec<Constraint>)>) -> Self {
        MockIntentResolver { rules }
    }
}

impl IntentResolver for MockIntentResolver {
    fn resolve_intent(&self, intent: &Intent) -> Result<IntentOutcome, IntentError> {
        match intent {
            // 透传 / 模板不需 AI。
            Intent::Explicit(cs) => Ok(IntentOutcome {
                constraints: cs.clone(),
                assist: AiAssist {
                    ai_involved: false,
                    model_id: "none".into(),
                    reason: "explicit constraints (no AI)".into(),
                },
            }),
            Intent::Template(name) => Err(IntentError::TranslateFailed(format!(
                "模板 {name} 未实现(模板 crate 待建)"
            ))),
            // 自然语言:关键词匹配规则(mock 的"AI")。
            Intent::NaturalLanguage(text) => {
                for (kw, cs) in &self.rules {
                    if text.contains(kw) {
                        return Ok(IntentOutcome {
                            constraints: cs.clone(),
                            assist: AiAssist {
                                ai_involved: true,
                                model_id: "mock".into(),
                                reason: format!("matched rule '{kw}'"),
                            },
                        });
                    }
                }
                Err(IntentError::TranslateFailed(format!(
                    "mock 无匹配规则: {text}"
                )))
            }
        }
    }
}

// ───────────────────────── DeepSeekResolver ─────────────────────────

/// 经系统 `curl` 调 DeepSeek 翻译自然语言意图(不引 Rust HTTP/JSON 依赖)。
///
/// key 从环境变量 `DEEPSEEK_API_KEY` 读(不硬编码、不入库)。
/// 提示模型**只输出逐行 `包名` 或 `包名>=版本`**,避免引 JSON 解析。
/// 模型/网络不可用 → [`IntentError::ModelUnavailable`],调用方降级(ADR-0005)。
pub struct DeepSeekResolver {
    api_key: String,
    model: String,
}

impl DeepSeekResolver {
    /// 从环境变量构造;无 key 返回 None(调用方据此降级到 Mock/Explicit)。
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("DEEPSEEK_API_KEY").ok()?;
        if api_key.trim().is_empty() {
            return None;
        }
        Some(DeepSeekResolver {
            api_key,
            model: "deepseek-chat".into(),
        })
    }

    /// 构造发给 DeepSeek 的提示(系统约束模型只回包名行)。
    fn build_prompt(intent: &str) -> String {
        format!(
            "你是 Debian 包依赖意图翻译器。用户描述一个软件环境意图,你输出满足该意图的 \
             Debian 顶层包名,每行一个,可带版本约束(格式 `包名` 或 `包名>=版本`)。\
             只输出包名行,不要解释、不要 markdown、不要代码块。包名用真实 Debian 包名(全小写)。\n\
             用户意图:{intent}"
        )
    }
}

impl IntentResolver for DeepSeekResolver {
    fn resolve_intent(&self, intent: &Intent) -> Result<IntentOutcome, IntentError> {
        let text = match intent {
            Intent::Explicit(cs) => {
                return Ok(IntentOutcome {
                    constraints: cs.clone(),
                    assist: AiAssist {
                        ai_involved: false,
                        model_id: "none".into(),
                        reason: "explicit constraints (no AI)".into(),
                    },
                })
            }
            Intent::Template(name) => {
                return Err(IntentError::TranslateFailed(format!("模板 {name} 未实现")))
            }
            Intent::NaturalLanguage(t) => t,
        };

        let raw = call_deepseek(&self.api_key, &self.model, &Self::build_prompt(text))?;
        let constraints = parse_constraints(&raw);
        if constraints.is_empty() {
            return Err(IntentError::TranslateFailed(format!(
                "DeepSeek 响应未解析出约束: {raw}"
            )));
        }
        Ok(IntentOutcome {
            constraints,
            assist: AiAssist {
                ai_involved: true,
                model_id: self.model.clone(),
                reason: format!("DeepSeek 翻译: {}", raw.replace('\n', ", ").trim()),
            },
        })
    }
}

/// 经系统 `curl` 调 DeepSeek chat completions,返回模型文本内容。
///
/// 不引 Rust HTTP/JSON 依赖(沿用项目 zstd/gunzip 的"调系统工具"套路)。
/// 请求体用手工 JSON(内容已转义);响应用极简提取(找 "content":" 后的字符串)。
fn call_deepseek(api_key: &str, model: &str, prompt: &str) -> Result<String, IntentError> {
    // 手工构造 JSON 请求体(prompt 做最小转义)。
    let escaped = json_escape(prompt);
    let body = format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":"{escaped}"}}],"stream":false,"max_tokens":256}}"#
    );

    // P1-17:Authorization 经 --config 文件(0600)传,不进 argv(防 /proc/pid/cmdline 泄露)。
    let auth = ai_client::AuthConfigFile::new("deepseek", &[format!("Authorization: Bearer {api_key}")])
        .map_err(IntentError::ModelUnavailable)?;
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s")
        .arg("--max-time")
        .arg("60")
        .arg("https://api.deepseek.com/chat/completions")
        .arg("-H")
        .arg("Content-Type: application/json");
    if let Some(a) = &auth {
        cmd.arg("--config").arg(&a.path);
    }
    let output = cmd
        .arg("-d")
        .arg(&body)
        .output()
        .map_err(|e| IntentError::ModelUnavailable(format!("curl 执行失败: {e}")))?;

    if !output.status.success() {
        return Err(IntentError::ModelUnavailable(format!(
            "curl 退出码 {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let resp = String::from_utf8_lossy(&output.stdout);
    extract_content(&resp)
}

/// 从 DeepSeek JSON 响应提取 `choices[0].message.content`。
/// 收敛到 [`ai_client::extract_json_string`](P1-14:此前 lib.rs 与 ai_client 各有一套
/// 分歧实现,都不解 `\uXXXX`;现统一到那个正确的逐字符 unescaper)。
fn extract_content(resp: &str) -> Result<String, IntentError> {
    ai_client::extract_json_string(resp, "\"content\":\"")
        .map_err(IntentError::TranslateFailed)
}

/// 最小 JSON 字符串转义(用于请求体的 prompt)。
/// 收敛到 [`ai_client::json_escape`](P1-16:含 `<0x20` 控制字符转义)。
fn json_escape(s: &str) -> String {
    ai_client::json_escape(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_constraint_variants() {
        let c = parse_constraint_line("python3>=3.10").unwrap();
        assert_eq!(c.name, "python3");
        assert_eq!(c.op, Some(VerOp::Ge));
        assert_eq!(c.ver.as_deref(), Some("3.10"));

        let u = parse_constraint_line("coreutils").unwrap();
        assert_eq!(u.name, "coreutils");
        assert_eq!(u.op, None);

        // 句子/解释行不被当成包名
        assert!(parse_constraint_line("这是一段解释").is_none());
        assert!(parse_constraint_line("# 注释").is_none());
        assert!(parse_constraint_line("").is_none());
    }

    #[test]
    fn mock_natural_language() {
        let r = MockIntentResolver::with_defaults();
        let out = r
            .resolve_intent(&Intent::NaturalLanguage("我要数据科学环境".into()))
            .unwrap();
        assert!(out.assist.ai_involved);
        assert_eq!(out.assist.model_id, "mock");
        let names: Vec<&str> = out.constraints.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"python3"));
        assert!(names.contains(&"numpy"));
    }

    #[test]
    fn mock_explicit_no_ai() {
        let r = MockIntentResolver::with_defaults();
        let cs = vec![Constraint::unconstrained("bash")];
        let out = r.resolve_intent(&Intent::Explicit(cs)).unwrap();
        assert!(!out.assist.ai_involved, "显式约束不应标记 AI 介入");
        assert_eq!(out.constraints[0].name, "bash");
    }

    #[test]
    fn mock_no_match_errs() {
        let r = MockIntentResolver::with_defaults();
        let res = r.resolve_intent(&Intent::NaturalLanguage("量子计算环境".into()));
        assert!(res.is_err(), "无匹配规则应 Err(调用方降级)");
    }

    #[test]
    fn extract_content_handles_escapes() {
        let resp = r#"{"choices":[{"message":{"content":"python3\nnumpy\npandas"}}]}"#;
        let c = extract_content(resp).unwrap();
        assert_eq!(c, "python3\nnumpy\npandas");
        let cs = parse_constraints(&c);
        assert_eq!(cs.len(), 3);
    }

    #[test]
    fn deepseek_from_env_none_without_key() {
        // 无 key 时 from_env 返回 None(调用方降级)。注:不依赖环境,只验证空串逻辑。
        // 真实 key 的端到端在 milestone6.rs(需 DEEPSEEK_API_KEY)。
        std::env::remove_var("DEEPSEEK_API_KEY");
        assert!(DeepSeekResolver::from_env().is_none());
    }
}
