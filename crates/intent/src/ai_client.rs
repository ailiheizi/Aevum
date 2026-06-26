//! 统一 AI 客户端:支持多模型(OpenAI/DeepSeek/Claude/Ollama),从配置文件读取。
//!
//! 所有 provider 走 OpenAI 兼容 chat completions API(curl 调用),
//! Claude 单独适配其 Messages API 格式。

use std::path::Path;

/// AI 配置(从 config.toml [ai] 段加载)。
#[derive(Debug, Clone)]
pub struct AiConfig {
    pub provider: String,
    pub api_key: String,
    pub model: String,
    pub endpoint: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub auto_repair: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: "deepseek".into(),
            api_key: String::new(),
            model: "deepseek-chat".into(),
            endpoint: "https://api.deepseek.com/chat/completions".into(),
            temperature: 0.3,
            max_tokens: 512,
            auto_repair: false,
        }
    }
}

impl AiConfig {
    /// 从 config.toml 文件加载 [ai] 段;不存在则回退环境变量 + 默认值。
    pub fn load(config_path: &Path) -> Self {
        let mut cfg = AiConfig::default();

        // 尝试读配置文件
        if let Ok(text) = std::fs::read_to_string(config_path) {
            if let Ok(doc) = aevum_service_compiler::parse_toml_subset(&text) {
                if let Some(ai) = doc.get("ai") {
                    if let Some(p) = ai.get_str("provider") {
                        cfg.provider = p;
                    }
                    if let Some(k) = ai.get_str("api_key") {
                        if !k.is_empty() {
                            cfg.api_key = k;
                        }
                    }
                    if let Some(m) = ai.get_str("model") {
                        if !m.is_empty() {
                            cfg.model = m;
                        }
                    }
                    if let Some(e) = ai.get_str("endpoint") {
                        if !e.is_empty() {
                            cfg.endpoint = e;
                        }
                    }
                    if let Some(t) = ai.get_str("temperature") {
                        cfg.temperature = t.parse().unwrap_or(0.3);
                    }
                    if let Some(t) = ai.get_str("max_tokens") {
                        cfg.max_tokens = t.parse().unwrap_or(512);
                    }
                    if let Some(r) = ai.get_str("auto_repair") {
                        cfg.auto_repair = r == "true";
                    }
                }
            }
        }

        // 根据 provider 设默认 endpoint/model(如果配置没显式指定)
        Self::apply_provider_defaults(&mut cfg);

        // 回退:从环境变量读 key
        if cfg.api_key.is_empty() {
            cfg.api_key = std::env::var("AEVUM_AI_KEY")
                .or_else(|_| match cfg.provider.as_str() {
                    "deepseek" => std::env::var("DEEPSEEK_API_KEY"),
                    "openai" => std::env::var("OPENAI_API_KEY"),
                    "claude" => std::env::var("ANTHROPIC_API_KEY"),
                    _ => std::env::var("AEVUM_AI_KEY"),
                })
                .unwrap_or_default();
        }

        cfg
    }

    fn apply_provider_defaults(cfg: &mut AiConfig) {
        // 只在 endpoint 还是默认 deepseek 值时才覆盖
        let is_default_endpoint = cfg.endpoint == "https://api.deepseek.com/chat/completions";
        match cfg.provider.as_str() {
            "openai" => {
                if is_default_endpoint {
                    cfg.endpoint = "https://api.openai.com/v1/chat/completions".into();
                }
                if cfg.model == "deepseek-chat" {
                    cfg.model = "gpt-4o-mini".into();
                }
            }
            "claude" => {
                if is_default_endpoint {
                    cfg.endpoint = "https://api.anthropic.com/v1/messages".into();
                }
                if cfg.model == "deepseek-chat" {
                    cfg.model = "claude-sonnet-4-20250514".into();
                }
            }
            "ollama" => {
                if is_default_endpoint {
                    cfg.endpoint = "http://localhost:11434/v1/chat/completions".into();
                }
                if cfg.model == "deepseek-chat" {
                    cfg.model = "llama3".into();
                }
            }
            _ => {} // deepseek 或 custom:保持默认
        }
    }

    /// 是否有可用的 AI(有 key 或是本地 ollama)。
    pub fn is_available(&self) -> bool {
        !self.api_key.is_empty() || self.provider == "ollama"
    }
}

/// 调用 AI 模型(统一接口,支持所有 provider)。
///
/// 返回模型输出的文本内容。使用系统 curl(零 Rust HTTP 依赖)。
pub fn ai_chat(config: &AiConfig, system_prompt: &str, user_message: &str) -> Result<String, String> {
    if !config.is_available() {
        return Err(format!(
            "AI 不可用:provider={}, 无 API key(设 AEVUM_AI_KEY 环境变量或在 config.toml 配置)",
            config.provider
        ));
    }

    if config.provider == "claude" {
        return call_claude(config, system_prompt, user_message);
    }

    // OpenAI 兼容 API(DeepSeek/OpenAI/Ollama 都用这个)
    call_openai_compat(config, system_prompt, user_message)
}

/// OpenAI 兼容 chat completions API。
fn call_openai_compat(config: &AiConfig, system_prompt: &str, user_message: &str) -> Result<String, String> {
    let sys_escaped = json_escape(system_prompt);
    let user_escaped = json_escape(user_message);
    let body = format!(
        r#"{{"model":"{}","messages":[{{"role":"system","content":"{}"}},{{"role":"user","content":"{}"}}],"stream":false,"max_tokens":{},"temperature":{}}}"#,
        config.model, sys_escaped, user_escaped, config.max_tokens, config.temperature
    );

    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s")
        .arg("--max-time")
        .arg(format!("{}", 60))
        .arg(&config.endpoint)
        .arg("-H")
        .arg("Content-Type: application/json");

    if !config.api_key.is_empty() {
        cmd.arg("-H")
            .arg(format!("Authorization: Bearer {}", config.api_key));
    }

    cmd.arg("-d").arg(&body);

    let output = cmd.output().map_err(|e| format!("curl 执行失败: {e}"))?;
    if !output.status.success() {
        return Err(format!("AI 请求失败: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let resp = String::from_utf8_lossy(&output.stdout);
    extract_openai_content(&resp)
}

/// Claude Messages API(格式不同于 OpenAI)。
fn call_claude(config: &AiConfig, system_prompt: &str, user_message: &str) -> Result<String, String> {
    let sys_escaped = json_escape(system_prompt);
    let user_escaped = json_escape(user_message);
    let body = format!(
        r#"{{"model":"{}","max_tokens":{},"system":"{}","messages":[{{"role":"user","content":"{}"}}]}}"#,
        config.model, config.max_tokens, sys_escaped, user_escaped
    );

    let output = std::process::Command::new("curl")
        .arg("-s")
        .arg("--max-time")
        .arg("60")
        .arg(&config.endpoint)
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-H")
        .arg(format!("x-api-key: {}", config.api_key))
        .arg("-H")
        .arg("anthropic-version: 2023-06-01")
        .arg("-d")
        .arg(&body)
        .output()
        .map_err(|e| format!("curl 执行失败: {e}"))?;

    if !output.status.success() {
        return Err(format!("Claude 请求失败: {}", String::from_utf8_lossy(&output.stderr)));
    }

    let resp = String::from_utf8_lossy(&output.stdout);
    // Claude 响应格式:{"content":[{"type":"text","text":"..."}]}
    extract_claude_content(&resp)
}

/// 从 OpenAI 兼容响应提取 content。
fn extract_openai_content(resp: &str) -> Result<String, String> {
    // 极简提取:找 "content":" 后的字符串(不引 serde_json)
    let marker = r#""content":""#;
    let Some(pos) = resp.find(marker) else {
        return Err(format!("AI 响应无 content 字段: {}", &resp[..resp.len().min(200)]));
    };
    let start = pos + marker.len();
    let rest = &resp[start..];
    // 找配对的 " (处理转义)
    let mut end = 0;
    let bytes = rest.as_bytes();
    while end < bytes.len() {
        if bytes[end] == b'"' && (end == 0 || bytes[end - 1] != b'\\') {
            break;
        }
        end += 1;
    }
    let content = &rest[..end];
    Ok(content.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// 从 Claude 响应提取 text。
fn extract_claude_content(resp: &str) -> Result<String, String> {
    let marker = r#""text":""#;
    let Some(pos) = resp.find(marker) else {
        return Err(format!("Claude 响应无 text 字段: {}", &resp[..resp.len().min(200)]));
    };
    let start = pos + marker.len();
    let rest = &resp[start..];
    let mut end = 0;
    let bytes = rest.as_bytes();
    while end < bytes.len() {
        if bytes[end] == b'"' && (end == 0 || bytes[end - 1] != b'\\') {
            break;
        }
        end += 1;
    }
    let content = &rest[..end];
    Ok(content.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// JSON 字符串转义(最小:处理 \、"、\n、\r、\t)。
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = AiConfig::default();
        assert_eq!(cfg.provider, "deepseek");
        assert_eq!(cfg.endpoint, "https://api.deepseek.com/chat/completions");
    }

    #[test]
    fn provider_defaults_applied() {
        let mut cfg = AiConfig::default();
        cfg.provider = "openai".into();
        AiConfig::apply_provider_defaults(&mut cfg);
        assert!(cfg.endpoint.contains("openai.com"));
        assert_eq!(cfg.model, "gpt-4o-mini");
    }

    #[test]
    fn extract_openai_response() {
        let resp = r#"{"choices":[{"message":{"content":"hello world"}}]}"#;
        assert_eq!(extract_openai_content(resp).unwrap(), "hello world");
    }

    #[test]
    fn extract_claude_response() {
        let resp = r#"{"content":[{"type":"text","text":"hello from claude"}]}"#;
        assert_eq!(extract_claude_content(resp).unwrap(), "hello from claude");
    }

    #[test]
    fn json_escape_works() {
        assert_eq!(json_escape("a\"b\nc"), r#"a\"b\nc"#);
    }
}
