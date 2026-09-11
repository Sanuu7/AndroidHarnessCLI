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
    pub workspace: PathBuf,
    pub messages: Vec<Msg>,
    /// Where it was saved, empty until the first save.
    pub path: PathBuf,
}

impl Session {
    pub fn new(model: &str, provider: &str) -> Self {
        let now = unix_now();
        Self {
            id: format!("{}-{}-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos(), std::process::id(), next_id()),
            title: String::new(),
            created: now,
            updated: now,
            model: model.to_string(),
            provider: provider.to_string(),
            workspace: std::env::current_dir().unwrap_or_default(),
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
            "workspace": self.workspace,
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
            workspace: v.get("workspace").and_then(|p| p.as_str()).unwrap_or("").into(),
            messages,
            path: path.clone(),
        })
    }

    pub fn matches_workspace(&self, root: &std::path::Path) -> bool {
        self.workspace.as_os_str().is_empty() ||
            self.workspace.canonicalize().unwrap_or_else(|_| self.workspace.clone()) ==
            root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
    }

    /// Complete the provider protocol without pretending an interrupted action succeeded.
    pub fn repair_interrupted(&mut self) {
        let old = std::mem::take(&mut self.messages);
        let mut pending: Vec<ToolCall> = Vec::new();
        for msg in old {
            if !matches!(msg.role, Role::Tool) {
                for call in pending.drain(..) {
                    self.messages.push(Msg::tool(&call, "Interrupted before a result was saved. The outcome is unknown. Inspect current state before retrying; do not repeat completed operations.", true));
                }
            }
            if matches!(msg.role, Role::Tool) {
                pending.retain(|c| c.id != msg.call_id);
            }
            pending.extend(msg.calls.clone());
            self.messages.push(msg);
        }
        for call in pending {
            self.messages.push(Msg::tool(&call, "Interrupted before a result was saved. The outcome is unknown. Inspect current state before retrying; do not repeat completed operations.", true));
        }
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

fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
    #[test]
    fn checkpoint_roundtrip_repairs_only_unknown_results() {
        let dir = crate::tools::testutil::temp_dir("recovery");
        let a = ToolCall { id: "a".into(), name: "write_file".into(), args: "{}".into() };
        let b = ToolCall { id: "b".into(), name: "shell".into(), args: "{}".into() };
        let mut session = Session::from_messages("model", "provider", vec![
            Msg::user("do work"), Msg::assistant("", vec![a.clone(), b.clone()]), Msg::tool(&a, "saved", false)
        ]);
        session.path = dir.join("session.json");
        session.workspace = dir.clone();
        session.save().unwrap();
        let mut restored = Session::load(&session.path).unwrap();
        restored.repair_interrupted();
        assert_eq!(restored.messages.len(), 4);
        assert_eq!(restored.messages[2].text, "saved");
        assert_eq!(restored.messages[3].call_id, "b");
        assert!(restored.messages[3].error);
        assert!(restored.messages[3].text.contains("unknown"));
        restored.repair_interrupted();
        assert_eq!(restored.messages.len(), 4, "recovery is idempotent");
        assert!(restored.matches_workspace(&dir));
        assert!(!restored.matches_workspace(&dir.join("other")));
        assert_eq!(restored.provider, "provider");
    }

    #[test]
    fn rapid_new_sessions_have_distinct_ids() {
        assert_ne!(Session::new("m", "p").id, Session::new("m", "p").id);
    }

    #[test]
    fn legacy_sessions_can_still_be_opened() {
        let dir = crate::tools::testutil::temp_dir("legacy-session");
        let path = dir.join("old.json");
        fs::write(&path, r#"{"messages":[],"id":"old"}"#).unwrap();
        assert!(Session::load(&path).unwrap().matches_workspace(&dir));
    }

}
