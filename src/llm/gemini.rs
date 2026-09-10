//! Google Gemini: streamGenerateContent with SSE, function calls inline in
//! the parts list rather than in a separate field.

use super::http::HttpReq;
use super::{Delta, Msg, Role, ToolCall, Turn, Wire, base_headers};
use crate::config::Provider;
use serde_json::{Value, json};

#[derive(Default)]
pub struct Gemini;

pub fn messages_json(messages: &[Msg]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::User => out.push(json!({
                "role": "user",
                "parts": [{ "text": m.text }],
            })),
            Role::Assistant => {
                let mut parts: Vec<Value> = Vec::new();
                if !m.text.trim().is_empty() {
                    parts.push(json!({ "text": m.text }));
                }
                for c in &m.calls {
                    parts.push(json!({
                        "functionCall": { "name": c.name, "args": parse_args(&c.args) },
                    }));
                }
                if parts.is_empty() {
                    parts.push(json!({ "text": "(no output)" }));
                }
                out.push(json!({ "role": "model", "parts": parts }));
            }
            Role::Tool => {
                let response = if m.error {
                    json!({ "error": m.text })
                } else {
                    json!({ "result": m.text })
                };
                let part = json!({
                    "functionResponse": { "name": m.name, "response": response },
                });
                // Gemini has no tool role: results ride in a user turn.
                match out.last_mut() {
                    Some(last) if last.get("role").and_then(|r| r.as_str()) == Some("user")
                        && last.get("parts").and_then(|p| p.as_array()).is_some_and(|p| {
                            p.iter().any(|x| x.get("functionResponse").is_some())
                        }) =>
                    {
                        if let Some(arr) = last.get_mut("parts").and_then(|p| p.as_array_mut()) {
                            arr.push(part);
                        }
                    }
                    _ => out.push(json!({ "role": "user", "parts": [part] })),
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

pub fn body(turn: &Turn) -> Value {
    let declarations: Vec<Value> = turn
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.desc,
                "parameters": t.params,
            })
        })
        .collect();
    let mut body = json!({
        "contents": messages_json(turn.messages),
        "systemInstruction": { "parts": [{ "text": turn.system }] },
        "generationConfig": { "maxOutputTokens": turn.max_tokens },
    });
    if !declarations.is_empty() {
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
    }
    // Gemini 2.x takes a thinking budget, and turns reasoning off with zero.
    if let Some(thinking) = turn.thinking {
        body["generationConfig"]["thinkingConfig"] = json!({ "thinkingBudget": thinking.budget });
    }
    body
}

impl Wire for Gemini {
    fn build(&self, provider: &Provider, turn: &Turn) -> HttpReq {
        let mut headers = base_headers();
        if !provider.api_key.is_empty() {
            headers.push(("x-goog-api-key".into(), provider.api_key.clone()));
        }
        let base = provider.base_url.trim_end_matches('/');
        HttpReq {
            url: format!("{base}/models/{}:streamGenerateContent?alt=sse", turn.model),
            headers,
            body: body(turn).to_string(),
        }
    }

    fn parse(&mut self, data: &str, out: &mut Delta) -> Result<(), String> {
        let v: Value = serde_json::from_str(data).map_err(|e| format!("bad json: {e}"))?;
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("gemini error");
            return Err(msg.to_string());
        }
        if let Some(usage) = v.get("usageMetadata") {
            out.usage = Some(super::Usage {
                input: super::as_u64(usage, "promptTokenCount"),
                output: super::as_u64(usage, "candidatesTokenCount")
                    + super::as_u64(usage, "thoughtsTokenCount"),
                cache_read: super::as_u64(usage, "cachedContentTokenCount"),
                cache_write: 0,
            });
        }
        let Some(candidate) = v
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        else {
            return Ok(());
        };
        if let Some(reason) = candidate.get("finishReason").and_then(|r| r.as_str()) {
            out.finish = Some(reason.to_ascii_lowercase());
        }
        let Some(parts) = candidate
            .get("content")
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
        else {
            return Ok(());
        };
        for (i, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                // Thought parts are flagged; everything else is the answer.
                if part.get("thought").and_then(|t| t.as_bool()).unwrap_or(false) {
                    out.reasoning.push_str(text);
                } else {
                    out.text.push_str(text);
                }
            }
            if let Some(call) = part.get("functionCall") {
                let name = call.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
                out.calls.push(ToolCall {
                    id: format!("gem_{i}_{name}"),
                    name: name.to_string(),
                    args: args.to_string(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_thought_parts_split() {
        let mut w = Gemini;
        let mut d = Delta::default();
        w.parse(
            r#"{"candidates":[{"content":{"parts":[{"text":"answer","thought":true},{"text":"real"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":2}}"#,
            &mut d,
        )
        .unwrap();
        assert_eq!(d.text, "real");
        assert_eq!(d.reasoning, "answer");
        assert_eq!(d.finish.as_deref(), Some("stop"));
        assert_eq!(d.usage.unwrap().input, 7);
    }

    #[test]
    fn function_calls_are_serialized_back_to_json_text() {
        let mut w = Gemini;
        let mut d = Delta::default();
        w.parse(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"bash","args":{"cmd":"ls"}}}]}}]}"#,
            &mut d,
        )
        .unwrap();
        assert_eq!(d.calls.len(), 1);
        assert_eq!(d.calls[0].name, "bash");
        assert_eq!(serde_json::from_str::<Value>(&d.calls[0].args).unwrap()["cmd"], json!("ls"));
    }

    #[test]
    fn tool_results_merge_into_one_user_turn() {
        let calls = vec![
            ToolCall { id: "1".into(), name: "bash".into(), args: "{}".into() },
            ToolCall { id: "2".into(), name: "grep".into(), args: "{}".into() },
        ];
        let msgs = vec![
            Msg::user("go"),
            Msg::assistant("", calls.clone()),
            Msg::tool(&calls[0], "one", false),
            Msg::tool(&calls[1], "two", true),
        ];
        let json = messages_json(&msgs);
        assert_eq!(json.len(), 3);
        let parts = json[2]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1]["functionResponse"]["response"]["error"], json!("two"));
    }

    #[test]
    fn url_targets_the_model_stream_endpoint() {
        let p = Provider {
            name: "g".into(),
            kind: crate::config::Kind::Gemini,
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            api_key: "k".into(),
            models: Vec::new(),
        };
        let t = Turn {
            model: "gemini-2.5-pro",
            system: "s",
            messages: &[],
            tools: &[],
            max_tokens: 100,
            thinking: None,
        };
        let req = Gemini.build(&p, &t);
        assert!(req.url.ends_with("/models/gemini-2.5-pro:streamGenerateContent?alt=sse"), "{}", req.url);
        assert!(req.headers.iter().any(|(k, _)| k == "x-goog-api-key"));
    }
}
