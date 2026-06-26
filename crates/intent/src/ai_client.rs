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

    #[test]
    fn chat_history_roundtrip() {
        let path = std::env::temp_dir().join(format!("aevum-hist-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut h = ChatHistory::load(&path);
        assert!(h.messages.is_empty());
        h.push("user", "我要 python\n环境");
        h.push("assistant", "好的");
        h.save(&path, 20);
        let h2 = ChatHistory::load(&path);
        assert_eq!(h2.messages.len(), 2);
        assert_eq!(h2.messages[0].content, "我要 python\n环境"); // 换行正确往返
        assert_eq!(h2.messages[1].role, "assistant");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn history_truncates_to_max() {
        let path = std::env::temp_dir().join(format!("aevum-hist-trunc-{}.txt", std::process::id()));
        let mut h = ChatHistory { messages: Vec::new() };
        for i in 0..30 {
            h.push("user", &format!("msg{i}"));
        }
        h.save(&path, 10);
        let h2 = ChatHistory::load(&path);
        assert_eq!(h2.messages.len(), 10);
        assert_eq!(h2.messages[0].content, "msg20"); // 保留最近 10 条
        let _ = std::fs::remove_file(&path);
    }

    // ── ai_dispatch 意图解析(用真实 DeepSeek 响应文本固化,离线、不触网)──

    #[test]
    fn parse_dispatch_install_intent() {
        // 真实 DeepSeek 对"我要 python 数据科学环境"的响应格式
        let resp = "INTENT: install\n\
            PACKAGES: python3 python3-pip python3-numpy python3-pandas jupyter-notebook\n\
            QUERY:\n\
            REPLY: 好的，我将为你安装 Python 3 以及数据科学常用库。";
        let a = parse_dispatch_response(resp);
        assert_eq!(a.intent, "install");
        assert!(a.packages.contains(&"python3-numpy".to_string()));
        assert!(a.packages.contains(&"jupyter-notebook".to_string()));
        assert!(a.query.is_empty());
        assert!(a.reply.contains("Python"));
    }

    #[test]
    fn parse_dispatch_explain_intent() {
        let resp = "INTENT: explain\n\
            PACKAGES:\n\
            QUERY:\n\
            REPLY: 这个错误通常是因为系统找不到动态库文件。";
        let a = parse_dispatch_response(resp);
        assert_eq!(a.intent, "explain");
        assert!(a.packages.is_empty(), "explain 不应有包名");
        assert!(a.reply.contains("动态库"));
    }

    #[test]
    fn parse_dispatch_list_intent() {
        let resp = "INTENT: list\nPACKAGES:\nQUERY:\nREPLY: 我来帮你查看已安装的软件包列表。";
        let a = parse_dispatch_response(resp);
        assert_eq!(a.intent, "list");
        assert!(a.packages.is_empty());
    }

    #[test]
    fn parse_dispatch_search_intent() {
        let resp = "INTENT: search\nPACKAGES:\nQUERY: 文本编辑器\nREPLY: 我帮你搜索文本编辑器相关的包。";
        let a = parse_dispatch_response(resp);
        assert_eq!(a.intent, "search");
        assert_eq!(a.query, "文本编辑器");
    }

    #[test]
    fn parse_dispatch_malformed_degrades_to_chat() {
        // AI 没按格式回复 → 整段当 reply,意图降级 chat(绝不误触发安装/卸载)
        let resp = "你好！我是 Aevum 助手，有什么可以帮你的吗？";
        let a = parse_dispatch_response(resp);
        assert_eq!(a.intent, "chat", "无格式响应必须降级 chat,不可误判副作用意图");
        assert!(a.packages.is_empty());
        assert_eq!(a.reply, resp);
    }

    // ── ai_evaluate_repair 决策解析(用真实 DeepSeek 4 场景响应固化)──

    #[test]
    fn parse_repair_plan_a() {
        // 场景3:libhttp 可放宽到 2.0(方案A 可解)
        let resp = "PLAN: A\n\
            ACTION: 放宽 libhttp 约束到 >=2.0\n\
            REASON: 仅调整约束即可解决冲突，无需更换包版本，风险最低。";
        let d = parse_repair_response(resp);
        assert_eq!(d.chosen_plan, "A");
        assert!(d.action.contains("libhttp"));
        assert!(d.reasoning.contains("风险最低"));
    }

    #[test]
    fn parse_repair_plan_c() {
        // 场景2:libfoo =1.0 vs =2.0,无交集 → 保留两份
        let resp = "PLAN: C\n\
            ACTION: 保留 libfoo 1.0 和 2.0 两份版本\n\
            REASON: libfoo 无法通过放宽约束共存，保留两份是安全可行的方案。";
        let d = parse_repair_response(resp);
        assert_eq!(d.chosen_plan, "C");
        assert!(d.action.contains("两份"));
    }

    #[test]
    fn parse_repair_plan_d() {
        // 场景4:三方精确版本互斥 → 告知用户决策
        let resp = "PLAN: D\n\
            ACTION: 告知用户 libcore 无共存版本，需手动取舍\n\
            REASON: 三个版本互不兼容且都是精确版本约束，只能由用户决策。";
        let d = parse_repair_response(resp);
        assert_eq!(d.chosen_plan, "D");
        assert!(d.reasoning.contains("用户决策"));
    }

    #[test]
    fn parse_repair_malformed_falls_back() {
        // AI 没按格式 → 整段当 reasoning,plan 默认 A,action 占位
        let resp = "我觉得你应该放宽约束试试看。";
        let d = parse_repair_response(resp);
        assert_eq!(d.chosen_plan, "A");
        assert_eq!(d.action, "见 AI 分析");
        assert_eq!(d.reasoning, resp);
    }
}

// ─────────────────────── AI 修复依赖冲突 ───────────────────────

/// AI 修复建议:AI 评估后选择的方案。
#[derive(Debug, Clone)]
pub struct AiRepairDecision {
    /// 选择的方案:"A"(放宽)/"B"(升级父包)/"C"(保留两份)/"D"(告知用户)
    pub chosen_plan: String,
    /// AI 的理由(人话解释)
    pub reasoning: String,
    /// 具体动作(如"放宽 libc6 到 2.35"或"升级 openssl 父包到 1.2")
    pub action: String,
}

/// 用 AI 评估依赖冲突,选择最优修复方案。
///
/// 输入:冲突诊断信息(conflicts + 各方案建议)。
/// 输出:AI 选的方案 + 理由 + 具体动作。
pub fn ai_evaluate_repair(
    config: &AiConfig,
    conflicts_desc: &str,
    suggestions_desc: &str,
) -> Result<AiRepairDecision, String> {
    let system_prompt = r#"你是 Aevum 包管理器的依赖冲突修复 AI。用户的系统出现了依赖版本冲突。
你需要从给出的修复方案中选择最优的一个并解释理由。

修复方案优先级(风险从低到高):
- A(放宽约束):最安全,只改约束不改包,推荐优先用
- B(升级父包):需要升级某些包到新版本,可能引入不兼容
- C(保留两份):占磁盘但安全,两个版本各跑各的
- D(告知用户):无法自动修复,需用户手动取舍

请用以下精确格式回复(每行一个字段):
PLAN: A|B|C|D
ACTION: 具体动作描述(如"放宽 libc6 约束到 >=2.35")
REASON: 一句话理由"#;

    let user_msg = format!(
        "依赖冲突:\n{conflicts_desc}\n\n可用修复方案:\n{suggestions_desc}\n\n请选择最优方案。"
    );

    let response = ai_chat(config, system_prompt, &user_msg)?;
    Ok(parse_repair_response(&response))
}

/// 把 AI 的修复决策文本解析成 [`AiRepairDecision`](纯函数,离线可测,不碰网络)。
///
/// 期望格式:`PLAN: A|B|C|D` / `ACTION:` / `REASON:`。
/// AI 没按格式(action 为空)→ 整个响应当 reasoning,plan 默认 A。
pub fn parse_repair_response(response: &str) -> AiRepairDecision {
    let mut plan = String::from("A");
    let mut action = String::new();
    let mut reasoning = String::new();

    for line in response.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("PLAN:") {
            plan = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("ACTION:") {
            action = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("REASON:") {
            reasoning = v.trim().to_string();
        }
    }

    if action.is_empty() {
        // AI 没按格式回复,把整个响应当 reasoning
        reasoning = response.to_string();
        action = "见 AI 分析".into();
    }

    AiRepairDecision {
        chosen_plan: plan,
        reasoning,
        action,
    }
}

/// 格式化冲突诊断为人/AI 可读的文本。
pub fn format_conflicts(conflicts: &[(String, String, String, String)]) -> String {
    // 每个 tuple: (package, chosen_version, source, required_constraint)
    let mut out = String::new();
    for (pkg, chosen, source, required) in conflicts {
        out.push_str(&format!("  {} 已选 {},但 {} 要求 {}\n", pkg, chosen, source, required));
    }
    out
}

/// 格式化修复建议为 AI 可读的文本。
pub fn format_suggestions(
    plan_a: &[(String, Option<String>)],   // (package, satisfying_version)
    plan_b: &[(String, String, String, String)], // (parent, upgrade_to, dep, dep_ver)
    plan_c: &[(String, String, String)],   // (package, ver_a, ver_b)
) -> String {
    let mut out = String::new();
    if !plan_a.is_empty() {
        out.push_str("方案A(放宽约束):\n");
        for (pkg, ver) in plan_a {
            match ver {
                Some(v) => out.push_str(&format!("  {} → 可放宽到 {}\n", pkg, v)),
                None => out.push_str(&format!("  {} → 无单一共存版本\n", pkg)),
            }
        }
    }
    if !plan_b.is_empty() {
        out.push_str("方案B(升级父包):\n");
        for (parent, upgrade_to, dep, dep_ver) in plan_b {
            out.push_str(&format!("  升级 {} 到 {} → {} 取 {}\n", parent, upgrade_to, dep, dep_ver));
        }
    }
    if !plan_c.is_empty() {
        out.push_str("方案C(保留两份):\n");
        for (pkg, va, vb) in plan_c {
            out.push_str(&format!("  {} 保留 {} 与 {} 两份\n", pkg, va, vb));
        }
    }
    if out.is_empty() {
        out.push_str("方案D:无自动修复方案,需用户取舍。\n");
    }
    out
}

// ─────────────────────── 统一 AI 入口:多轮对话 + 意图分发 ───────────────────────

/// 一条对话消息。
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,    // "user" | "assistant"
    pub content: String,
}

/// 对话历史(存盘,跨命令接续)。极简文本格式:每行 `role\tcontent`(content 转义换行)。
pub struct ChatHistory {
    pub messages: Vec<ChatMessage>,
}

impl ChatHistory {
    /// 从历史文件加载;不存在则空。
    pub fn load(path: &Path) -> Self {
        let mut messages = Vec::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                if let Some((role, content)) = line.split_once('\t') {
                    messages.push(ChatMessage {
                        role: role.to_string(),
                        content: content.replace("\\n", "\n"),
                    });
                }
            }
        }
        ChatHistory { messages }
    }

    /// 保存(只保留最近 max 条)。
    pub fn save(&self, path: &Path, max: usize) {
        let start = self.messages.len().saturating_sub(max);
        let mut out = String::new();
        for m in &self.messages[start..] {
            out.push_str(&format!("{}\t{}\n", m.role, m.content.replace('\n', "\\n")));
        }
        let _ = std::fs::write(path, out);
    }

    pub fn push(&mut self, role: &str, content: &str) {
        self.messages.push(ChatMessage { role: role.to_string(), content: content.to_string() });
    }

    pub fn reset(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

/// AI 判断的意图动作。
#[derive(Debug, Clone)]
pub struct AiAction {
    /// install | explain | repair | list | search | remove | gc | chat
    pub intent: String,
    /// install/remove 时的包名
    pub packages: Vec<String>,
    /// search 时的关键词
    pub query: String,
    /// 给用户的回复
    pub reply: String,
}

/// 统一 AI 入口:给定历史 + 当前输入,AI 判断意图并返回结构化动作。
///
/// AI 被要求输出结构化文本(INTENT/PACKAGES/QUERY/REPLY),CLI 据此分发。
pub fn ai_dispatch(config: &AiConfig, history: &[ChatMessage], user_input: &str) -> Result<AiAction, String> {
    let system_prompt = r#"你是 Aevum 包管理器的统一 AI 助手。用户用自然语言对话,你判断意图并回复。

意图类型:
- install: 用户想安装软件/搭环境(如"我要 python 环境")
- remove: 用户想卸载包
- search: 用户想搜索包
- list: 用户想看已装了什么
- gc: 用户想清理旧世代
- explain: 用户问为什么出错/某概念
- repair: 用户问依赖冲突怎么解决
- chat: 闲聊或其它

用以下精确格式回复(每个字段一行):
INTENT: <上述类型之一>
PACKAGES: <空格分隔的真实 Debian 包名,仅 install/remove 时;否则留空>
QUERY: <搜索词,仅 search 时;否则留空>
REPLY: <给用户的中文回复,一两句话说明你要做什么或解答>

注意:install 时 PACKAGES 必须是真实 Debian 包名(全小写,如 python3 numpy git ripgrep)。
结合对话历史理解"刚才""再加上"等指代。"#;

    // 拼接历史 + 当前输入为多轮 messages
    let mut messages: Vec<ChatMessage> = history.to_vec();
    messages.push(ChatMessage { role: "user".into(), content: user_input.to_string() });

    let response = ai_chat_history(config, system_prompt, &messages)?;
    Ok(parse_dispatch_response(&response))
}

/// 把 AI 的结构化文本响应解析成 [`AiAction`](纯函数,离线可测,不碰网络)。
///
/// 期望格式(每字段一行):`INTENT:` / `PACKAGES:` / `QUERY:` / `REPLY:`。
/// AI 没按格式 → 整个响应当 reply,意图降级为 chat(不会误触发副作用动作)。
pub fn parse_dispatch_response(response: &str) -> AiAction {
    let mut action = AiAction {
        intent: "chat".into(),
        packages: Vec::new(),
        query: String::new(),
        reply: String::new(),
    };
    for line in response.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("INTENT:") {
            action.intent = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("PACKAGES:") {
            action.packages = v.split_whitespace().map(|s| s.to_string()).collect();
        } else if let Some(v) = line.strip_prefix("QUERY:") {
            action.query = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("REPLY:") {
            action.reply = v.trim().to_string();
        }
    }
    // AI 没按格式 → 整个响应当 reply,意图 chat
    if action.reply.is_empty() {
        action.reply = response.to_string();
    }
    action
}

/// 多轮对话:把历史 messages 全部塞进 API 请求(OpenAI 兼容 / Claude 都支持)。
pub fn ai_chat_history(config: &AiConfig, system_prompt: &str, messages: &[ChatMessage]) -> Result<String, String> {
    if !config.is_available() {
        return Err(format!("AI 不可用(provider={}, 无 key)", config.provider));
    }
    if config.provider == "claude" {
        return call_claude_history(config, system_prompt, messages);
    }
    // OpenAI 兼容:system + 历史 messages
    let mut msg_json = format!(r#"{{"role":"system","content":"{}"}}"#, json_escape(system_prompt));
    for m in messages {
        msg_json.push_str(&format!(
            r#",{{"role":"{}","content":"{}"}}"#,
            m.role, json_escape(&m.content)
        ));
    }
    let body = format!(
        r#"{{"model":"{}","messages":[{}],"stream":false,"max_tokens":{},"temperature":{}}}"#,
        config.model, msg_json, config.max_tokens, config.temperature
    );

    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s").arg("--max-time").arg("60")
        .arg(&config.endpoint)
        .arg("-H").arg("Content-Type: application/json");
    if !config.api_key.is_empty() {
        cmd.arg("-H").arg(format!("Authorization: Bearer {}", config.api_key));
    }
    cmd.arg("-d").arg(&body);
    let output = cmd.output().map_err(|e| format!("curl 失败: {e}"))?;
    if !output.status.success() {
        return Err(format!("AI 请求失败: {}", String::from_utf8_lossy(&output.stderr)));
    }
    extract_openai_content(&String::from_utf8_lossy(&output.stdout))
}

/// Claude 多轮(system 单独字段,messages 数组)。
fn call_claude_history(config: &AiConfig, system_prompt: &str, messages: &[ChatMessage]) -> Result<String, String> {
    let mut msg_json = String::new();
    for (i, m) in messages.iter().enumerate() {
        if i > 0 { msg_json.push(','); }
        msg_json.push_str(&format!(r#"{{"role":"{}","content":"{}"}}"#, m.role, json_escape(&m.content)));
    }
    let body = format!(
        r#"{{"model":"{}","max_tokens":{},"system":"{}","messages":[{}]}}"#,
        config.model, config.max_tokens, json_escape(system_prompt), msg_json
    );
    let output = std::process::Command::new("curl")
        .arg("-s").arg("--max-time").arg("60")
        .arg(&config.endpoint)
        .arg("-H").arg("Content-Type: application/json")
        .arg("-H").arg(format!("x-api-key: {}", config.api_key))
        .arg("-H").arg("anthropic-version: 2023-06-01")
        .arg("-d").arg(&body)
        .output().map_err(|e| format!("curl 失败: {e}"))?;
    if !output.status.success() {
        return Err(format!("Claude 请求失败: {}", String::from_utf8_lossy(&output.stderr)));
    }
    extract_claude_content(&String::from_utf8_lossy(&output.stdout))
}
