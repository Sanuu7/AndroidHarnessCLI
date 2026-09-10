//! OpenAI's Responses API (`POST /v1/responses`). Event-shaped rather than
//! chunk-shaped, and tool calls arrive as separate items.

use super::http::HttpReq;
use super::{Delta, Msg, Role, ToolCall, Turn, Wire, base_headers, endpoint};
use crate::config::Provider;
use serde_json::{Value, json};

#[derive(Default)]
pub struct Responses {
    /// item_id -> call, while its arguments stream in.
    pending: Vec<(String, ToolCall)>,
}

pub fn input_json(messages: &[Msg]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::User => out.push(json!({
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": m.text }],
            })),
            Role::Assistant => {
                if !m.text.trim().is_empty() {
                    out.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": m.text }],
                    }));
                }
                for c in &m.calls {
                    out.push(json!({
                        "type": "function_call",
                        "call_id": c.id,
                        "name": c.name,
                        "arguments": c.args,
                    }));
                }
            }
            Role::Tool => out.push(json!({
                "type": "function_call_output",
                "call_id": m.call_id,
                "output": m.text,
            })),
        }
    }
    out
}

pub fn body(turn: &Turn) -> Value {
    let tools: Vec<Value> = turn
        .tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "name": t.name,
                "description": t.desc,
                "parameters": t.params,
            })
        })
        .collect();
    let mut body = json!({
        "model": turn.model,
        "instructions": turn.system,
        "input": input_json(turn.messages),
        "max_output_tokens": turn.max_tokens,
        "stream": true,
        "store": false,
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    body
}

impl Responses {
    fn slot(&mut self, id: &str) -> &mut ToolCall {
        if let Some(pos) = self.pending.iter().position(|(k, _)| k == id) {
            return &mut self.pending[pos].1;
        }
        self.pending.push((id.to_string(), ToolCall::default()));
        let last = self.pending.len() - 1;
        &mut self.pending[last].1
    }
}

impl Wire for Responses {
    fn build(&self, provider: &Provider, turn: &Turn) -> HttpReq {
        let mut headers = base_headers();
        if !provider.api_key.is_empty() {
            headers.push(("Authorization".into(), format!("Bearer {}", provider.api_key)));
        }
        HttpReq {
            url: endpoint(provider, "/responses"),
            headers,
            body: body(turn).to_string(),
        }
    }

    fn parse(&mut self, data: &str, out: &mut Delta) -> Result<(), String> {
        if data.trim() == "[DONE]" {
            return Ok(());
        }
        let v: Value = serde_json::from_str(data).map_err(|e| format!("bad json: {e}"))?;
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match kind {
            "error" | "response.failed" => {
                let msg = v
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .or_else(|| {
                        v.get("response")
                            .and_then(|r| r.get("error"))
                            .and_then(|e| e.get("message"))
                            .and_then(|m| m.as_str())
                    })
                    .unwrap_or("responses error");
                return Err(msg.to_string());
            }
            "response.output_text.delta" => {
                if let Some(t) = v.get("delta").and_then(|d| d.as_str()) {
                    out.text.push_str(t);
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(t) = v.get("delta").and_then(|d| d.as_str()) {
                    out.reasoning.push_str(t);
                }
            }
            "response.output_item.added" => {
                if let Some(item) = v.get("item") {
                    if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                        let id = item
                            .get("id")
                            .and_then(|i| i.as_str())
                            .or_else(|| item.get("call_id").and_then(|i| i.as_str()))
                            .unwrap_or("")
                            .to_string();
                        let call = self.slot(&id);
                        call.id = item
                            .get("call_id")
                            .and_then(|c| c.as_str())
                            .unwrap_or(&id)
                            .to_string();
                        call.name = item.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let id = v
                    .get("item_id")
                    .and_then(|i| i.as_str())
                    .or_else(|| v.get("call_id").and_then(|i| i.as_str()))
                    .unwrap_or("");
                if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                    self.slot(id).args.push_str(delta);
                }
            }
            "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                        let id = item
                            .get("id")
                            .and_then(|i| i.as_str())
                            .or_else(|| item.get("call_id").and_then(|i| i.as_str()))
                            .unwrap_or("")
                            .to_string();
                        let call_id =
                            item.get("call_id").and_then(|c| c.as_str()).unwrap_or(&id).to_string();
                        let name =
                            item.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let args = item
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .map(|s| s.to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| self.slot(&id).args.clone());
                        if !name.is_empty() {
                            out.calls.push(ToolCall { id: call_id, name, args });
                        }
                        self.pending.retain(|(k, _)| k != &id);
                    }
                }
            }
            "response.completed" => {
                if let Some(usage) = v.get("response").and_then(|r| r.get("usage")) {
                    out.usage = Some(super::Usage {
                        input: super::as_u64(usage, "input_tokens"),
                        output: super::as_u64(usage, "output_tokens"),
                        cache_read: usage
                            .get("input_tokens_details")
                            .map(|d| super::as_u64(d, "cached_tokens"))
                            .unwrap_or(0),
                        cache_write: 0,
                    });
                }
                out.finish = Some("stop".into());
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut Delta) {
        for (_, call) in self.pending.drain(..) {
            if !call.name.is_empty() {
                out.calls.push(call);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_deltas_and_usage() {
        let mut w = Responses::default();
        let mut d = Delta::default();
        w.parse(r#"{"type":"response.output_text.delta","delta":"hey"}"#, &mut d).unwrap();
        w.parse(
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":9,"output_tokens":3,"input_tokens_details":{"cached_tokens":5}}}}"#,
            &mut d,
        )
        .unwrap();
        assert_eq!(d.text, "hey");
        let u = d.usage.unwrap();
        assert_eq!((u.input, u.output, u.cache_read), (9, 3, 5));
    }

    #[test]
    fn function_calls_collect_across_events() {
        let mut w = Responses::default();
        let mut d = Delta::default();
        w.parse(
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"i1","call_id":"c1","name":"bash","arguments":""}}"#,
            &mut d,
        )
        .unwrap();
        w.parse(
            r#"{"type":"response.function_call_arguments.delta","item_id":"i1","delta":"{\"cmd\":\"pwd\"}"}"#,
            &mut d,
        )
        .unwrap();
        w.parse(
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"i1","call_id":"c1","name":"bash","arguments":"{\"cmd\":\"pwd\"}"}}"#,
            &mut d,
        )
        .unwrap();
        assert_eq!(d.calls.len(), 1);
        assert_eq!(d.calls[0].id, "c1");
        assert_eq!(d.calls[0].args, r#"{"cmd":"pwd"}"#);
    }

    #[test]
    fn input_items_use_function_call_output() {
        let call = ToolCall { id: "c1".into(), name: "read_file".into(), args: "{}".into() };
        let msgs = vec![
            Msg::user("read it"),
            Msg::assistant("sure", vec![call.clone()]),
            Msg::tool(&call, "contents", false),
        ];
        let items = input_json(&msgs);
        assert_eq!(items[0]["content"][0]["type"], json!("input_text"));
        assert_eq!(items[1]["type"], json!("message"), "assistant text is its own item");
        assert_eq!(items[2]["type"], json!("function_call"));
        assert_eq!(items[3]["type"], json!("function_call_output"));
        assert_eq!(items[3]["call_id"], json!("c1"));
    }

    #[test]
    fn failed_responses_raise() {
        let mut w = Responses::default();
        let mut d = Delta::default();
        let err = w
            .parse(r#"{"type":"response.failed","response":{"error":{"message":"quota"}}}"#, &mut d)
            .unwrap_err();
        assert_eq!(err, "quota");
    }
}
