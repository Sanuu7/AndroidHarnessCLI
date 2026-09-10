//! Anthropic /v1/messages. Blocks, not roles: tool results travel inside a
//! user turn, which is the main shape difference to keep in mind here.

use super::http::HttpReq;
use super::{Delta, Msg, Role, ToolCall, Turn, Wire, base_headers, endpoint};
use crate::config::Provider;
use serde_json::{Value, json};

pub const VERSION: &str = "2023-06-01";

#[derive(Default)]
struct Block {
    kind: String,
    call: ToolCall,
}

#[derive(Default)]
pub struct Anthropic {
    current: Option<Block>,
    input_tokens: u64,
}

/// Anthropic wants tool results grouped into one user message, and the system
/// prompt out of band.
pub fn messages_json(messages: &[Msg]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::User => out.push(json!({
                "role": "user",
                "content": [{ "type": "text", "text": m.text }],
            })),
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                if !m.text.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": m.text }));
                }
                for c in &m.calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": c.id,
                        "name": c.name,
                        "input": parse_args(&c.args),
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({ "type": "text", "text": "(no output)" }));
                }
                out.push(json!({ "role": "assistant", "content": blocks }));
            }
            Role::Tool => {
                let result = json!({
                    "type": "tool_result",
                    "tool_use_id": m.call_id,
                    "content": m.text,
                    "is_error": m.error,
                });
                // Merge into the previous user turn when it is already results.
                match out.last_mut() {
                    Some(last)
                        if last.get("role").and_then(|r| r.as_str()) == Some("user")
                            && last
                                .get("content")
                                .and_then(|c| c.as_array())
                                .and_then(|a| a.first())
                                .and_then(|b| b.get("type"))
                                .and_then(|t| t.as_str())
                                == Some("tool_result") =>
                    {
                        if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                            arr.push(result);
                        }
                    }
                    _ => out.push(json!({ "role": "user", "content": [result] })),
                }
            }
        }
    }
    out
}

fn parse_args(args: &str) -> Value {
    if args.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(args).unwrap_or_else(|_| json!({}))
}

pub fn body(provider: &Provider, turn: &Turn, stream: bool) -> Value {
    let _ = provider;
    let tools: Vec<Value> = turn
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.desc,
                "input_schema": t.params,
            })
        })
        .collect();
    let mut body = json!({
        "model": turn.model,
        "system": turn.system,
        "messages": messages_json(turn.messages),
        "max_tokens": turn.max_tokens,
        "stream": stream,
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    body
}

impl Wire for Anthropic {
    fn build(&self, provider: &Provider, turn: &Turn) -> HttpReq {
        let mut headers = base_headers();
        headers.push(("anthropic-version".into(), VERSION.into()));
        if !provider.api_key.is_empty() {
            headers.push(("x-api-key".into(), provider.api_key.clone()));
        }
        let base = provider.base_url.clone();
        // Both https://api.anthropic.com and .../v1 are accepted in config.
        let path = if base.ends_with("/v1") { "/messages" } else { "/v1/messages" };
        HttpReq {
            url: endpoint(provider, path),
            headers,
            body: body(provider, turn, true).to_string(),
        }
    }

    fn parse(&mut self, data: &str, out: &mut Delta) -> Result<(), String> {
        let v: Value = serde_json::from_str(data).map_err(|e| format!("bad json: {e}"))?;
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match kind {
            "error" => {
                let msg = v
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("anthropic error");
                return Err(msg.to_string());
            }
            "message_start" => {
                if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                    self.input_tokens = super::as_u64(usage, "input_tokens");
                    out.usage = Some(super::Usage {
                        input: self.input_tokens,
                        output: 0,
                        cache_read: super::as_u64(usage, "cache_read_input_tokens"),
                        cache_write: super::as_u64(usage, "cache_creation_input_tokens"),
                    });
                }
            }
            "content_block_start" => {
                let block = v.get("content_block").cloned().unwrap_or(Value::Null);
                let btype = block.get("type").and_then(|t| t.as_str()).unwrap_or("text").to_string();
                let call = ToolCall {
                    id: block.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                    name: block.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                    args: String::new(),
                };
                self.current = Some(Block { kind: btype, call });
            }
            "content_block_delta" => {
                let Some(delta) = v.get("delta") else { return Ok(()) };
                match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text_delta" => {
                        if let Some(t) = delta.get("text").and_then(|t| t.as_str()) {
                            out.text.push_str(t);
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = delta.get("thinking").and_then(|t| t.as_str()) {
                            out.reasoning.push_str(t);
                        }
                    }
                    "input_json_delta" => {
                        if let (Some(partial), Some(block)) = (
                            delta.get("partial_json").and_then(|p| p.as_str()),
                            self.current.as_mut(),
                        ) {
                            block.call.args.push_str(partial);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(block) = self.current.take() {
                    if block.kind == "tool_use" && !block.call.name.is_empty() {
                        out.calls.push(block.call);
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = v
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|r| r.as_str())
                {
                    out.finish = Some(reason.to_string());
                }
                if let Some(usage) = v.get("usage") {
                    let output = super::as_u64(usage, "output_tokens");
                    if output > 0 {
                        let prev = out.usage.unwrap_or_default();
                        out.usage = Some(super::Usage {
                            input: if prev.input > 0 { prev.input } else { self.input_tokens },
                            output,
                            ..prev
                        });
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(w: &mut Anthropic, events: &[&str]) -> Delta {
        let mut d = Delta::default();
        for e in events {
            w.parse(e, &mut d).unwrap();
        }
        d
    }

    #[test]
    fn text_blocks_stream() {
        let mut w = Anthropic::default();
        let d = parse_all(
            &mut w,
            &[
                r#"{"type":"message_start","message":{"usage":{"input_tokens":12,"cache_read_input_tokens":4}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
            ],
        );
        assert_eq!(d.text, "hi");
        assert_eq!(d.reasoning, "hmm");
        assert_eq!(d.finish.as_deref(), Some("end_turn"));
        let u = d.usage.unwrap();
        assert_eq!(u.input, 12);
        assert_eq!(u.output, 3);
        assert_eq!(u.cache_read, 4);
    }

    #[test]
    fn tool_use_input_streams_as_json() {
        let mut w = Anthropic::default();
        let d = parse_all(
            &mut w,
            &[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
            ],
        );
        assert_eq!(d.calls.len(), 1);
        assert_eq!(d.calls[0].id, "toolu_1");
        assert_eq!(d.calls[0].name, "bash");
        assert_eq!(d.calls[0].args, r#"{"cmd":"ls"}"#);
    }

    #[test]
    fn tool_results_merge_into_one_user_turn() {
        let calls = vec![
            ToolCall { id: "a".into(), name: "bash".into(), args: "{}".into() },
            ToolCall { id: "b".into(), name: "read_file".into(), args: "{}".into() },
        ];
        let msgs = vec![
            Msg::user("go"),
            Msg::assistant("", calls.clone()),
            Msg::tool(&calls[0], "one", false),
            Msg::tool(&calls[1], "two", false),
        ];
        let json = messages_json(&msgs);
        assert_eq!(json.len(), 3, "two results collapse into one user turn");
        let blocks = json[2]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1]["content"], json!("two"));
        assert_eq!(json[1]["content"][0]["input"], json!({}));
    }

    #[test]
    fn errors_come_back_as_messages() {
        let mut w = Anthropic::default();
        let mut d = Delta::default();
        let err = w
            .parse(r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#, &mut d)
            .unwrap_err();
        assert_eq!(err, "busy");
    }

    #[test]
    fn base_url_with_or_without_v1_lands_on_messages() {
        for base in ["https://api.anthropic.com", "https://api.anthropic.com/v1"] {
            let p = Provider {
                name: "a".into(),
                kind: crate::config::Kind::Anthropic,
                base_url: base.into(),
                api_key: "k".into(),
                models: Vec::new(),
            };
            let t = Turn {
                model: "claude-sonnet-4-5",
                system: "s",
                messages: &[],
                tools: &[],
                max_tokens: 10,
            };
            let req = Anthropic::default().build(&p, &t);
            assert!(req.url.ends_with("/v1/messages"), "{}", req.url);
            assert!(req.headers.iter().any(|(k, _)| k == "x-api-key"));
        }
    }
}
