//! Sessions on disk.
//!
//! One JSON file per conversation under `~/.local/share/harness/sessions/`,
//! written after every turn so a crash costs at most one exchange. The format
//! is the message list plus enough metadata to resume it.

use crate::config::Config;
use crate::llm::{Msg, Role, ToolCall};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Session {
    pub id: String,
    pub title: String,
    pub created: u64,
    pub updated: u64,
    pub model: String,
    pub provider: String,
    pub messages: Vec<Msg>,
    /// Where it was saved, empty until the first save.
    pub path: PathBuf,
}

impl Session {
    pub fn new(model: &str, provider: &str) -> Self {
        let now = unix_now();
        Self {
            id: format!("{now}"),
            title: String::new(),
            created: now,
            updated: now,
            model: model.to_string(),
            provider: provider.to_string(),
            messages: Vec::new(),
            path: PathBuf::new(),
        }
    }

    pub fn dir() -> PathBuf {
        Config::data_dir().join("sessions")
    }

    /// Build one around an existing conversation (resume, fork, tests).
    #[cfg(test)]
    pub fn from_messages(model: &str, provider: &str, messages: Vec<Msg>) -> Self {
        let mut s = Session::new(model, provider);
        s.title = title_from(&messages);
        s.messages = messages;
        s
    }

    pub fn save(&mut self) -> std::io::Result<PathBuf> {
        self.updated = unix_now();
        if self.title.is_empty() {
            self.title = title_from(&self.messages);
        }
        if self.path.as_os_str().is_empty() {
            let dir = Session::dir();
            fs::create_dir_all(&dir)?;
            self.path = dir.join(format!("{}.json", self.id));
        }
        let value = json!({
            "id": self.id,
            "title": self.title,
            "created": self.created,
            "updated": self.updated,
            "model": self.model,
            "provider": self.provider,
            "messages": self.messages.iter().map(message_json).collect::<Vec<_>>(),
        });
        let text = serde_json::to_string(&value).unwrap_or_else(|_| "{}".into());
        crate::config::write_private(&self.path, &text)?;
        Ok(self.path.clone())
    }

    pub fn load(path: &PathBuf) -> Option<Session> {
        let text = fs::read_to_string(path).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        let messages = v
            .get("messages")?
            .as_array()?
            .iter()
            .filter_map(message_from)
            .collect();
        Some(Session {
            id: v.get("id").and_then(|i| i.as_str()).unwrap_or("0").to_string(),
            title: v.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            created: v.get("created").and_then(|c| c.as_u64()).unwrap_or(0),
            updated: v.get("updated").and_then(|u| u.as_u64()).unwrap_or(0),
            model: v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string(),
            provider: v.get("provider").and_then(|p| p.as_str()).unwrap_or("").to_string(),
            messages,
            path: path.clone(),
        })
    }

    /// Most recently updated session, for `harness --continue`.
    pub fn latest() -> Option<PathBuf> {
        let dir = Session::dir();
        let mut best: Option<(u64, PathBuf)> = None;
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e != "json").unwrap_or(true) {
                continue;
            }
            let stamp = fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v.get("updated").and_then(|u| u.as_u64()))
                .unwrap_or(0);
            if best.as_ref().map(|(b, _)| stamp > *b).unwrap_or(true) {
                best = Some((stamp, path));
            }
        }
        best.map(|(_, p)| p)
    }

    /// Newest first, for `/sessions`.
    pub fn list() -> Vec<(String, String, u64, PathBuf)> {
        let mut out: Vec<(String, String, u64, PathBuf)> = Vec::new();
        let Ok(entries) = fs::read_dir(Session::dir()) else { return out };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(session) = Session::load(&path) else { continue };
            out.push((session.id, session.title, session.updated, path));
        }
        out.sort_by(|a, b| b.2.cmp(&a.2));
        out
    }
}

pub fn title_from(messages: &[Msg]) -> String {
    messages
        .iter()
        .find(|m| m.role == Role::User)
        .map(|m| {
            let first = m.text.lines().next().unwrap_or("").trim().to_string();
            first.chars().take(60).collect()
        })
        .unwrap_or_else(|| "new session".into())
}

pub fn message_json(m: &Msg) -> Value {
    json!({
        "role": match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        },
        "text": m.text,
        "calls": m.calls.iter().map(|c| json!({
            "id": c.id, "name": c.name, "args": c.args,
        })).collect::<Vec<_>>(),
        "call_id": m.call_id,
        "name": m.name,
        "error": m.error,
    })
}

pub fn message_from(v: &Value) -> Option<Msg> {
    let role = match v.get("role")?.as_str()? {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => Role::Tool,
    };
    let calls = v
        .get("calls")
        .and_then(|c| c.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(ToolCall {
                        id: c.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                        name: c.get("name").and_then(|n| n.as_str())?.to_string(),
                        args: c.get("args").and_then(|a| a.as_str()).unwrap_or("{}").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Msg {
        role,
        text: v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
        calls,
        call_id: v.get("call_id").and_then(|c| c.as_str()).unwrap_or("").to_string(),
        name: v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
        error: v.get("error").and_then(|e| e.as_bool()).unwrap_or(false),
    })
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_through_json() {
        let call = ToolCall { id: "c1".into(), name: "bash".into(), args: "{\"cmd\":\"ls\"}".into() };
        let msgs = vec![
            Msg::user("hi"),
            Msg::assistant("sure", vec![call.clone()]),
            Msg::tool(&call, "output", true),
        ];
        let json: Vec<Value> = msgs.iter().map(message_json).collect();
        let text = serde_json::to_string(&json).unwrap();
        let back: Vec<Value> = serde_json::from_str(&text).unwrap();
        let parsed: Vec<Msg> = back.iter().filter_map(message_from).collect();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1].calls[0].name, "bash");
        assert_eq!(parsed[2].call_id, "c1");
        assert!(parsed[2].error);
    }

    #[test]
    fn titles_come_from_the_first_user_message() {
        let msgs = vec![Msg::user("fix the build\nand the tests"), Msg::assistant("ok", vec![])];
        assert_eq!(title_from(&msgs), "fix the build");
        assert_eq!(title_from(&[]), "new session");
    }

    #[test]
    fn titles_are_bounded() {
        let long = "x".repeat(200);
        let title = title_from(&[Msg::user(long)]);
        assert_eq!(title.chars().count(), 60);
    }

    #[test]
    fn sessions_save_and_load_from_a_temp_data_dir() {
        let dir = crate::tools::testutil::temp_dir("sessions");
        // Point the data dir at the temp folder for this test.
        unsafe { std::env::set_var("HARNESS_DATA_DIR", &dir) };
        let mut session = Session::from_messages("m", "p", vec![Msg::user("hello there")]);
        let path = session.save().unwrap();
        assert!(path.exists());
        let loaded = Session::load(&path).expect("reload");
        assert_eq!(loaded.title, "hello there");
        assert_eq!(loaded.messages.len(), 1);
        let latest = Session::latest();
        assert_eq!(latest, Some(path.clone()));
        assert_eq!(Session::list().len(), 1);
        unsafe { std::env::remove_var("HARNESS_DATA_DIR") };
    }
}
