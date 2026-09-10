//! Talking to models.
//!
//! One [`Wire`] per HTTP dialect. A wire builds the request body and folds
//! the response stream into the same handful of deltas, so the agent loop
//! never learns which vendor it is talking to.

pub mod anthropic;
pub mod gemini;
pub mod http;
pub mod models;
pub mod openai;
pub mod relay;
pub mod responses;

use crate::config::{Kind, Provider};
use serde_json::{Value, json};

/// How hard the model should think before answering.
///
/// Same vocabulary pi and opencode use, so the levels mean the same thing
/// across providers. Each wire maps them the way it can: a token budget for
/// Anthropic and Gemini, an effort name for OpenAI-shaped APIs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Level {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl Level {
    pub fn all() -> [Level; 7] {
        [Level::Off, Level::Minimal, Level::Low, Level::Medium, Level::High, Level::XHigh, Level::Max]
    }

    /// Levels that make sense when the model has no reasoning at all.
    pub fn is_off(self) -> bool {
        self == Level::Off
    }

    pub fn parse(s: &str) -> Option<Level> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "disabled" => Level::Off,
            "minimal" | "min" => Level::Minimal,
            "low" => Level::Low,
            "medium" | "med" | "default" | "on" => Level::Medium,
            "high" => Level::High,
            "xhigh" | "x-high" | "extra" => Level::XHigh,
            "max" | "maximum" => Level::Max,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Minimal => "minimal",
            Level::Low => "low",
            Level::Medium => "medium",
            Level::High => "high",
            Level::XHigh => "xhigh",
            Level::Max => "max",
        }
    }

    /// Setting another level with one key press.
    pub fn next(self) -> Level {
        match self {
            Level::Off => Level::Minimal,
            Level::Minimal => Level::Low,
            Level::Low => Level::Medium,
            Level::Medium => Level::High,
            Level::High => Level::XHigh,
            Level::XHigh => Level::Max,
            Level::Max => Level::Off,
        }
    }

    /// Default token budget per level, in pi's numbers.
    pub fn budget(self) -> u32 {
        match self {
            Level::Off => 0,
            Level::Minimal => 1_024,
            Level::Low => 2_048,
            Level::Medium => 8_192,
            Level::High => 16_384,
            Level::XHigh => 32_768,
            Level::Max => 65_536,
        }
    }

    /// The effort name OpenAI-shaped APIs take. Levels they do not know are
    /// clamped rather than dropped, the way pi clamps xhigh and max.
    pub fn effort(self) -> Option<&'static str> {
        Some(match self {
            Level::Off => return None,
            Level::Minimal => "minimal",
            Level::Low => "low",
            Level::Medium => "medium",
            Level::High | Level::XHigh | Level::Max => "high",
        })
    }

    /// One line explaining what the level does, for the picker.
    pub fn describe(self) -> &'static str {
        match self {
            Level::Off => "no reasoning",
            Level::Minimal => "barely any reasoning (~1k tokens)",
            Level::Low => "light reasoning (~2k tokens)",
            Level::Medium => "moderate reasoning (~8k tokens)",
            Level::High => "deep reasoning (~16k tokens)",
            Level::XHigh => "deeper still (~32k tokens)",
            Level::Max => "as much as it wants",
        }
    }
}

/// What the request should ask for, if anything.
#[derive(Clone, Copy, Debug)]
pub struct Thinking {
    pub level: Level,
    /// Token budget, already trimmed to leave room for the answer.
    pub budget: u32,
}

impl Thinking {
    pub fn new(level: Level, max_tokens: u32) -> Option<Thinking> {
        if level.is_off() {
            return None;
        }
        // Keep at least a kilobyte of tokens for the answer itself, which is
        // what both pi and the Anthropic API require.
        let room = max_tokens.saturating_sub(MIN_ANSWER_TOKENS);
        let budget = level.budget().min(room.max(1_024));
        Some(Thinking { level, budget })
    }
}

/// Answer room that must survive whatever the thinking budget takes.
pub const MIN_ANSWER_TOKENS: u32 = 1_024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON text as the model wrote it.
    pub args: String,
}

#[derive(Clone, Debug)]
pub struct Msg {
    pub role: Role,
    pub text: String,
    pub calls: Vec<ToolCall>,
    /// Set on tool results: which call they answer.
    pub call_id: String,
    pub name: String,
    pub error: bool,
}

impl Msg {
    pub fn user(text: impl Into<String>) -> Msg {
        Msg {
            role: Role::User,
            text: text.into(),
            calls: Vec::new(),
            call_id: String::new(),
            name: String::new(),
            error: false,
        }
    }

    pub fn assistant(text: impl Into<String>, calls: Vec<ToolCall>) -> Msg {
        Msg {
            role: Role::Assistant,
            text: text.into(),
            calls,
            call_id: String::new(),
            name: String::new(),
            error: false,
        }
    }

    pub fn tool(call: &ToolCall, text: impl Into<String>, error: bool) -> Msg {
        Msg {
            role: Role::Tool,
            text: text.into(),
            calls: Vec::new(),
            call_id: call.id.clone(),
            name: call.name.clone(),
            error,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolSchema {
    pub name: String,
    pub desc: String,
    pub params: Value,
}

impl ToolSchema {
    pub fn new(name: &str, desc: &str, params: Value) -> Self {
        Self { name: name.into(), desc: desc.into(), params }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// What one chunk of the stream told us.
#[derive(Default)]
pub struct Delta {
    pub text: String,
    pub reasoning: String,
    /// Calls that finished inside this chunk.
    pub calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
    pub finish: Option<String>,
}

/// Everything a request needs, whatever the wire.
pub struct Turn<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub messages: &'a [Msg],
    pub tools: &'a [ToolSchema],
    pub max_tokens: u32,
    /// None when the model should not reason at all.
    pub thinking: Option<Thinking>,
}

pub trait Wire: Send {
    fn build(&self, provider: &Provider, turn: &Turn) -> http::HttpReq;
    /// Fold one SSE `data:` payload. Returning Err aborts the stream with a
    /// message the user gets to read.
    fn parse(&mut self, data: &str, out: &mut Delta) -> Result<(), String>;
    /// The stream ended: release anything still held back.
    fn finish(&mut self, _out: &mut Delta) {}
}

pub fn wire_for(kind: Kind) -> Box<dyn Wire> {
    match kind {
        // A relay is routed to a concrete wire by `relay::route` before it
        // gets here; chat/completions is the sane default if it does not.
        Kind::OpenAi | Kind::Relay => Box::new(openai::OpenAi::new()),
        Kind::Anthropic => Box::new(anthropic::Anthropic::default()),
        Kind::Gemini => Box::new(gemini::Gemini),
        Kind::Responses => Box::new(responses::Responses::default()),
    }
}

// -- shared helpers --------------------------------------------------------

/// Headers every JSON POST carries.
pub fn base_headers() -> Vec<(String, String)> {
    vec![
        ("Content-Type".into(), "application/json".into()),
        ("Accept".into(), "text/event-stream".into()),
        ("User-Agent".into(), format!("AndroidHarness/{}", env!("CARGO_PKG_VERSION"))),
    ]
}

pub fn endpoint(provider: &Provider, path: &str) -> String {
    format!("{}{}", provider.base_url.trim_end_matches('/'), path)
}

/// Pull the text out of an error body, whatever shape the vendor uses.
pub fn error_from_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "empty response".into();
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if let Some(msg) = v
            .get("error")
            .and_then(|e| e.get("message").or_else(|| e.get("type")).or(Some(e)))
            .and_then(|m| m.as_str())
        {
            return msg.to_string();
        }
        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
    }
    trimmed.lines().next().unwrap_or("request failed").chars().take(300).collect()
}

pub fn as_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(|n| n.as_u64()).unwrap_or(0)
}

/// A JSON-schema object for a tool, written once and shared by every wire.
pub fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

pub fn string_prop(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

pub fn number_prop(desc: &str) -> Value {
    json!({ "type": "integer", "description": desc })
}

pub fn bool_prop(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_bodies_reduce_to_one_line() {
        assert_eq!(error_from_body("{\"error\":{\"message\":\"bad key\"}}"), "bad key");
        assert_eq!(error_from_body("{\"message\":\"nope\"}"), "nope");
        assert_eq!(error_from_body("plain text\nsecond"), "plain text");
        assert_eq!(error_from_body("   "), "empty response");
    }

    #[test]
    fn endpoint_joins_without_doubling_slashes() {
        let p = Provider {
            name: "x".into(),
            kind: Kind::OpenAi,
            base_url: "https://api.openai.com/v1/".into(),
            api_key: String::new(),
            models: Vec::new(),
        };
        assert_eq!(endpoint(&p, "/chat/completions"), "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn tool_messages_carry_their_call_id() {
        let call = ToolCall { id: "c1".into(), name: "bash".into(), args: "{}".into() };
        let msg = Msg::tool(&call, "ok", false);
        assert_eq!(msg.call_id, "c1");
        assert_eq!(msg.name, "bash");
        assert_eq!(msg.role, Role::Tool);
    }

    #[test]
    fn levels_parse_and_cycle() {
        assert_eq!(Level::parse("HIGH"), Some(Level::High));
        assert_eq!(Level::parse("x-high"), Some(Level::XHigh));
        assert_eq!(Level::parse("nope"), None);
        for level in Level::all() {
            assert_eq!(Level::parse(level.as_str()), Some(level));
        }
        let mut level = Level::Off;
        for expected in
            [Level::Minimal, Level::Low, Level::Medium, Level::High, Level::XHigh, Level::Max, Level::Off]
        {
            level = level.next();
            assert_eq!(level, expected);
        }
    }

    #[test]
    fn effort_clamps_to_what_apis_know() {
        assert_eq!(Level::Off.effort(), None);
        assert_eq!(Level::Medium.effort(), Some("medium"));
        assert_eq!(Level::XHigh.effort(), Some("high"), "unknown levels clamp, not break");
        assert_eq!(Level::Max.effort(), Some("high"));
    }

    #[test]
    fn budgets_leave_room_for_the_answer() {
        let thinking = Thinking::new(Level::Max, 8_192).unwrap();
        assert_eq!(thinking.budget, 8_192 - MIN_ANSWER_TOKENS);
        let roomy = Thinking::new(Level::Low, 100_000).unwrap();
        assert_eq!(roomy.budget, 2_048);
        assert!(Thinking::new(Level::Off, 8_192).is_none());
    }
}
