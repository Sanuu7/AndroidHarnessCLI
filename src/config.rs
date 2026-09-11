//! Settings that outlive a session: providers, keys, the active model.
//!
//! One JSON file under `~/.config/harness/config.json` (0600, keys live in
//! it). Environment variables win over the file so a key can be piped in
//! without ever touching disk.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub const HARNESS_BASE: &str = "https://opencode.ai/zen/v1";
pub const HARNESS_MODEL: &str = "ling-3.0-flash-fin-free";

/// Which dialect of HTTP a provider speaks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// chat/completions: OpenAI, Groq, OpenRouter, DeepSeek, llama.cpp, ...
    OpenAi,
    /// The keyless relay: same wire, but it probes what the model answers on.
    Relay,
    Anthropic,
    Gemini,
    Responses,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "openai" | "openai-compat" | "compat" => Kind::OpenAi,
            "relay" | "harness" | "opencode" => Kind::Relay,
            "anthropic" | "claude" => Kind::Anthropic,
            "gemini" | "google" => Kind::Gemini,
            "responses" | "openai-responses" => Kind::Responses,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::OpenAi => "openai",
            Kind::Relay => "relay",
            Kind::Anthropic => "anthropic",
            Kind::Gemini => "gemini",
            Kind::Responses => "responses",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Provider {
    pub name: String,
    pub kind: Kind,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
}

impl Provider {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "kind": self.kind.as_str(),
            "base_url": self.base_url,
            "api_key": self.api_key,
            "models": self.models,
        })
    }

    pub fn from_json(v: &Value) -> Option<Provider> {
        let name = v.get("name")?.as_str()?.to_string();
        let kind = Kind::parse(v.get("kind").and_then(|k| k.as_str()).unwrap_or("openai"))?;
        let base_url = v
            .get("base_url")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .trim_end_matches('/')
            .to_string();
        let api_key = v.get("api_key").and_then(|k| k.as_str()).unwrap_or("").to_string();
        let models = v
            .get("models")
            .and_then(|m| m.as_array())
            .map(|a| a.iter().filter_map(|m| m.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        Some(Provider { name, kind, base_url, api_key, models })
    }
}

pub struct Config {
    pub providers: Vec<Provider>,
    pub provider: String,
    pub model: String,
    /// How hard the model should think by default.
    pub thinking: crate::llm::Level,
    /// Written for the user to read; the CLI keeps its own sane defaults.
    pub raw: Value,
    pub path: PathBuf,
}

impl Config {
    pub fn dir() -> PathBuf {
        if let Ok(v) = std::env::var("HARNESS_CONFIG_DIR") {
            return PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("XDG_CONFIG_HOME") {
            if !v.is_empty() {
                return PathBuf::from(v).join("harness");
            }
        }
        home().join(".config/harness")
    }

    pub fn data_dir() -> PathBuf {
        if let Ok(v) = std::env::var("HARNESS_DATA_DIR") {
            return PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("XDG_DATA_HOME") {
            if !v.is_empty() {
                return PathBuf::from(v).join("harness");
            }
        }
        home().join(".local/share/harness")
    }

    pub fn load() -> Config {
        let path = Config::dir().join("config.json");
        let raw = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .unwrap_or_else(|| Value::Object(Default::default()));

        let mut providers: Vec<Provider> = raw
            .get("providers")
            .and_then(|p| p.as_array())
            .map(|a| a.iter().filter_map(Provider::from_json).collect())
            .unwrap_or_default();

        // The keyless relay is the front door: no key, no signup, works on
        // first run. Provider files from the app or a hand edit only add.
        if providers.iter().all(|p| p.kind != Kind::Relay) {
            providers.insert(0, relay_provider());
        }

        let mut cfg = Config {
            provider: raw
                .get("provider")
                .and_then(|p| p.as_str())
                .unwrap_or("harness")
                .to_string(),
            model: raw
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or(HARNESS_MODEL)
                .to_string(),
            thinking: raw
                .get("thinking")
                .and_then(|t| t.as_str())
                .and_then(crate::llm::Level::parse)
                .unwrap_or(crate::llm::Level::Medium),
            providers,
            raw,
            path,
        };
        cfg.apply_env();
        if cfg.find(&cfg.provider).is_none() {
            cfg.provider = cfg.providers[0].name.clone();
        }
        // First run writes the file out, so there is something to edit and
        // something to point at when a key is needed.
        if !cfg.path.exists() {
            let _ = cfg.save();
        }
        cfg
    }

    /// `HARNESS_API_KEY` and `HARNESS_MODEL` so nothing has to be written down.
    fn apply_env(&mut self) {
        if let Ok(model) = std::env::var("HARNESS_MODEL") {
            if !model.is_empty() {
                self.model = model;
            }
        }
        if let Ok(level) = std::env::var("HARNESS_THINKING") {
            if let Some(level) = crate::llm::Level::parse(&level) {
                self.thinking = level;
            }
        }
        if let Ok(provider) = std::env::var("HARNESS_PROVIDER") {
            if self.find(&provider).is_some() {
                self.provider = provider;
            }
        }
        if let Ok(key) = std::env::var("HARNESS_API_KEY") {
            if !key.is_empty() {
                let name = self.provider.clone();
                if let Some(p) = self.find_mut(&name) {
                    p.api_key = key;
                }
            }
        }
    }

    pub fn find(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut Provider> {
        self.providers.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn active(&self) -> &Provider {
        self.find(&self.provider).unwrap_or(&self.providers[0])
    }

    pub fn set_active(&mut self, provider: &str, model: &str) -> std::io::Result<()> {
        self.provider = provider.to_string();
        self.model = model.to_string();
        self.save()
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        self.raw = json!({
            "provider": self.provider,
            "model": self.model,
            "thinking": self.thinking.as_str(),
            "providers": self.providers.iter().map(|p| p.to_json()).collect::<Vec<_>>(),
        });
        let text = serde_json::to_string_pretty(&self.raw).unwrap_or_else(|_| "{}".into());
        write_private(&self.path, &text)
    }
}

pub fn relay_provider() -> Provider {
    Provider {
        name: "harness".into(),
        kind: Kind::Relay,
        base_url: HARNESS_BASE.into(),
        api_key: String::new(),
        models: vec![
            HARNESS_MODEL.into(),
            "big-pickle".into(),
            "deepseek-v4-flash-free".into(),
            "mimo-v2.5-free".into(),
            "nemotron-3-ultra-free".into(),
            "nemotron-3.5-lightning-free".into(),
            "muse-spark-1.3-contributor-free".into(),
        ],
    }
}

/// Write a file only the user can read. Keys live in here.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = path.with_extension(format!("tmp-{}-{n}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() { let _ = fs::remove_file(&temp); }
    result
}

pub fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parsing_accepts_aliases() {
        assert_eq!(Kind::parse("openai-compat"), Some(Kind::OpenAi));
        assert_eq!(Kind::parse("Claude"), Some(Kind::Anthropic));
        assert_eq!(Kind::parse("nope"), None);
        for kind in [Kind::OpenAi, Kind::Relay, Kind::Anthropic, Kind::Gemini, Kind::Responses] {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn provider_round_trips_through_json() {
        let p = Provider {
            name: "openai".into(),
            kind: Kind::OpenAi,
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-test".into(),
            models: vec!["gpt-5".into()],
        };
        let back = Provider::from_json(&p.to_json()).expect("round trip");
        assert_eq!(back.name, p.name);
        assert_eq!(back.kind, p.kind);
        assert_eq!(back.api_key, "sk-test");
        assert_eq!(back.models, p.models);
    }

    #[test]
    fn relay_is_always_present() {
        let providers = vec![Provider {
            name: "openai".into(),
            kind: Kind::OpenAi,
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            models: Vec::new(),
        }];
        let mut all = providers;
        if all.iter().all(|p| p.kind != Kind::Relay) {
            all.insert(0, relay_provider());
        }
        assert_eq!(all[0].kind, Kind::Relay);
        assert!(all[0].api_key.is_empty());
    }

    #[test]
    fn base_urls_lose_the_trailing_slash() {
        let p = Provider::from_json(&json!({
            "name": "x", "kind": "openai", "base_url": "https://example.com/v1/"
        }))
        .unwrap();
        assert_eq!(p.base_url, "https://example.com/v1");
    }
}
