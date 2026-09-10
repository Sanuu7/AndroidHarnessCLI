//! Talking to models.
//!
//! One [`Wire`] per HTTP dialect. A wire builds the request body and folds
//! the response stream into the same handful of deltas, so the agent loop
//! never learns which vendor it is talking to.

pub mod anthropic;
pub mod gemini;
pub mod http;
pub mod openai;
pub mod relay;
pub mod responses;

use crate::config::{Kind, Provider};
use serde_json::{Value, json};

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
}
