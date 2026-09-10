//! OpenAI chat/completions. Also speaks for Groq, DeepSeek, OpenRouter,
//! Moonshot, llama.cpp, Ollama, LM Studio, and anything else that copied the
//! shape, which is most things.

use super::http::HttpReq;
use super::{Delta, Msg, Role, ToolCall, Turn, Wire, base_headers, endpoint};
use crate::config::Provider;
use serde_json::{Value, json};

#[derive(Default)]
pub struct OpenAi {
    /// Tool calls stream in pieces: index -> (id, name, args so far).
    partial: Vec<ToolCall>,
    sent: usize,
}

impl OpenAi {
    pub fn new() -> Self {
        Self::default()
    }
}

fn local_host(base_url: &str) -> bool {
    let host = base_url.to_ascii_lowercase();
    host.contains("127.0.0.1")
        || host.contains("localhost")
        || host.contains("0.0.0.0")
        || host.contains("192.168.")
        || host.contains("10.0.")
        || host.contains("[::1]")
}

fn wants_completion_tokens(base_url: &str, model: &str) -> bool {
    if !base_url.contains("api.openai.com") {
        return false;
    }
    let m = model.to_ascii_lowercase();
    m.starts_with("gpt-5") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

pub fn messages_json(messages: &[Msg]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        match m.role {
            Role::User => out.push(json!({ "role": "user", "content": m.text })),
            Role::Assistant => {
                if m.calls.is_empty() {
                    out.push(json!({ "role": "assistant", "content": m.text }));
                } else {
                    let calls: Vec<Value> = m
                        .calls
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            json!({
                                "id": if c.id.is_empty() { format!("call_{i}") } else { c.id.clone() },
                                "type": "function",
                                "function": { "name": c.name, "arguments": c.args },
                            })
                        })
                        .collect();
                    out.push(json!({ "role": "assistant", "content": m.text, "tool_calls": calls }));
                }
            }
            Role::Tool => out.push(json!({
                "role": "tool",
                "tool_call_id": m.call_id,
                "content": m.text,
            })),
        }
    }
    out
}

pub fn tools_json(tools: &[super::ToolSchema]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": { "name": t.name, "description": t.desc, "parameters": t.params },
            })
        })
        .collect()
}

pub fn body(provider: &Provider, turn: &Turn, stream: bool) -> Value {
    let mut body = json!({
        "model": turn.model,
        "messages": messages_json(turn.messages),
        "stream": stream,
    });
    if wants_completion_tokens(&provider.base_url, turn.model) {
        body["max_completion_tokens"] = json!(turn.max_tokens);
    } else {
        body["max_tokens"] = json!(turn.max_tokens);
    }
    if stream && !local_host(&provider.base_url) {
        // Ask for a usage block at the end; the status bar needs the numbers.
        body["stream_options"] = json!({ "include_usage": true });
    }
    if !turn.tools.is_empty() {
        body["tools"] = json!(tools_json(turn.tools));
        body["tool_choice"] = json!("auto");
    }
    body
}

impl Wire for OpenAi {
    fn build(&self, provider: &Provider, turn: &Turn) -> HttpReq {
        let mut headers = base_headers();
        if !provider.api_key.is_empty() {
            headers.push(("Authorization".into(), format!("Bearer {}", provider.api_key)));
        }
        HttpReq {
            url: endpoint(provider, "/chat/completions"),
            headers,
            body: body(provider, turn, true).to_string(),
        }
    }

    fn parse(&mut self, data: &str, out: &mut Delta) -> Result<(), String> {
        if data.trim() == "[DONE]" {
            out.finish = Some("stop".into());
            return Ok(());
        }
        let v: Value = serde_json::from_str(data).map_err(|e| format!("bad json: {e}"))?;
        if let Some(err) = v.get("error") {
            return Err(error_text(err));
        }
        if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
            out.usage = Some(super::Usage {
                input: super::as_u64(usage, "prompt_tokens"),
                output: super::as_u64(usage, "completion_tokens"),
                cache_read: usage
                    .get("prompt_tokens_details")
                    .map(|d| super::as_u64(d, "cached_tokens"))
                    .unwrap_or(0),
                cache_write: 0,
            });
        }
        let Some(choice) = v.get("choices").and_then(|c| c.as_array()).and_then(|a| a.first())
        else {
            return Ok(());
        };
        if let Some(reason) = choice.get("finish_reason").and_then(|f| f.as_str()) {
            if !reason.is_empty() && reason != "null" {
                out.finish = Some(reason.to_string());
            }
        }
        let Some(delta) = choice.get("delta") else { return Ok(()) };
        if let Some(text) = delta.get("content").and_then(|c| c.as_str()) {
            out.text.push_str(text);
        }
        // Reasoning models hide the thinking in a side channel; keep it.
        for key in ["reasoning_content", "reasoning"] {
            if let Some(think) = delta.get(key).and_then(|c| c.as_str()) {
                out.reasoning.push_str(think);
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(|c| c.as_array()) {
            for call in calls {
                let idx = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                while self.partial.len() <= idx {
                    self.partial.push(ToolCall::default());
                }
                let slot = &mut self.partial[idx];
                if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                    if !id.is_empty() {
                        slot.id = id.to_string();
                    }
                }
                if let Some(f) = call.get("function") {
                    if let Some(name) = f.get("name").and_then(|n| n.as_str()) {
                        if !name.is_empty() {
                            slot.name = name.to_string();
                        }
                    }
                    if let Some(args) = f.get("arguments").and_then(|a| a.as_str()) {
                        slot.args.push_str(args);
                    }
                }
            }
        }
        // A finished call is one whose arguments already parse as JSON, or
        // anything still held back once the stream says it is done.
        self.flush(false, out);
        Ok(())
    }

    fn finish(&mut self, out: &mut Delta) {
        self.flush(true, out);
    }
}

impl OpenAi {
    /// Move accumulated calls out once they look complete.
    fn flush(&mut self, force: bool, out: &mut Delta) {
        let ready = self
            .partial
            .iter()
            .take_while(|c| !c.name.is_empty() && (force || serde_json::from_str::<Value>(&c.args).is_ok()))
            .count();
        for call in self.partial.drain(..ready) {
            self.sent += 1;
            out.calls.push(call);
        }
        if force {
            for call in self.partial.drain(..) {
                if !call.name.is_empty() {
                    self.sent += 1;
                    out.calls.push(call);
                }
            }
        }
    }
}

fn error_text(err: &Value) -> String {
    err.get("message")
        .and_then(|m| m.as_str())
        .or_else(|| err.as_str())
        .unwrap_or("provider error")
        .to_string()
}

/// Build a request against a concrete provider (kept out of the trait so the
/// trait stays object-safe and vendor-free).
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_reasoning_deltas_accumulate() {
        let mut w = OpenAi::new();
        let mut d = Delta::default();
        w.parse(
            r#"{"choices":[{"delta":{"content":"he","reasoning_content":"hm"}}]}"#,
            &mut d,
        )
        .unwrap();
        w.parse(r#"{"choices":[{"delta":{"content":"llo"}}]}"#, &mut d).unwrap();
        assert_eq!(d.text, "hello");
        assert_eq!(d.reasoning, "hm");
    }

    #[test]
    fn tool_calls_arrive_in_pieces() {
        let mut w = OpenAi::new();
        let mut d = Delta::default();
        w.parse(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"bash","arguments":"{\"cmd\":"}}]}}]}"#,
            &mut d,
        )
        .unwrap();
        assert!(d.calls.is_empty(), "arguments are not valid json yet");
        w.parse(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
            &mut d,
        )
        .unwrap();
        assert_eq!(d.calls.len(), 1);
        assert_eq!(d.calls[0].name, "bash");
        assert_eq!(d.calls[0].id, "c1");
        assert_eq!(d.calls[0].args, r#"{"cmd":"ls"}"#);
    }

    #[test]
    fn usage_and_finish_are_read() {
        let mut w = OpenAi::new();
        let mut d = Delta::default();
        w.parse(
            r#"{"choices":[{"finish_reason":"stop","delta":{}}],"usage":{"prompt_tokens":10,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":6}}}"#,
            &mut d,
        )
        .unwrap();
        assert_eq!(d.finish.as_deref(), Some("stop"));
        let u = d.usage.unwrap();
        assert_eq!(u.input, 10);
        assert_eq!(u.output, 4);
        assert_eq!(u.cache_read, 6);
    }

    #[test]
    fn errors_surface() {
        let mut w = OpenAi::new();
        let mut d = Delta::default();
        let err = w.parse(r#"{"error":{"message":"bad key","type":"auth"}}"#, &mut d).unwrap_err();
        assert_eq!(err, "bad key");
    }

    #[test]
    fn request_body_carries_tools_and_systemless_messages() {
        let provider = Provider {
            name: "p".into(),
            kind: crate::config::Kind::OpenAi,
            base_url: "https://api.openai.com/v1".into(),
            api_key: "k".into(),
            models: Vec::new(),
        };
        let msgs = vec![
            Msg::user("hi"),
            Msg::assistant("", vec![ToolCall { id: "c1".into(), name: "bash".into(), args: "{}".into() }]),
            Msg::tool(&ToolCall { id: "c1".into(), name: "bash".into(), args: "{}".into() }, "ok", false),
        ];
        let t = Turn {
            model: "gpt-5-mini",
            system: "sys",
            messages: &msgs,
            tools: &[super::super::ToolSchema::new("bash", "run a command", json!({"type":"object"}))],
            max_tokens: 256,
        };
        let body = body(&provider, &t, true);
        assert!(body.get("max_completion_tokens").is_some(), "gpt-5 wants the newer field");
        assert_eq!(body["stream_options"]["include_usage"], json!(true));
        assert_eq!(body["messages"][2]["tool_call_id"], json!("c1"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("bash"));
    }

    #[test]
    fn local_hosts_skip_usage_accounting() {
        assert!(local_host("http://127.0.0.1:8080/v1"));
        assert!(!local_host("https://api.groq.com/openai/v1"));
    }

    #[test]
    fn completion_tokens_field_is_openai_only() {
        assert!(wants_completion_tokens("https://api.openai.com/v1", "o3-mini"));
        assert!(!wants_completion_tokens("https://api.groq.com/openai/v1", "gpt-5"));
    }
}
