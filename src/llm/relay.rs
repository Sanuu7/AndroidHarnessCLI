//! The keyless relay.
//!
//! No signup, no key: the same catalog is served behind three wires, and
//! which one a given model answers on is not advertised. So the first request
//! for a model tries chat/completions, then /v1/messages, then /responses,
//! and the winner is remembered in pins.json.

use super::http::{HttpMsg, HttpReq, post};
use super::base_headers;
use crate::config::{Config, Kind, Provider, write_private};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const SESSION_HEADER: &str = "x-opencode-session";
pub const KEYLESS: &str = "harness-keyless";
pub const REFERER: &str = "https://github.com/Sanuu7/AndroidHarnessCLI";

fn cache() -> &'static Mutex<BTreeMap<String, Kind>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Kind>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(load_pins()))
}

fn pins_path() -> std::path::PathBuf {
    Config::dir().join("pins.json")
}

fn load_pins() -> BTreeMap<String, Kind> {
    let mut out = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(pins_path()) else { return out };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return out };
    if let Some(map) = v.as_object() {
        for (model, wire) in map {
            if let Some(kind) = wire.as_str().and_then(Kind::parse) {
                out.insert(model.clone(), kind);
            }
        }
    }
    out
}

fn save_pins(map: &BTreeMap<String, Kind>) {
    let value: Value = map
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.as_str().to_string())))
        .collect::<serde_json::Map<_, _>>()
        .into();
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        if let Some(dir) = pins_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = write_private(&pins_path(), &text);
    }
}

pub fn pinned(model: &str) -> Option<Kind> {
    cache().lock().ok().and_then(|m| m.get(model).copied())
}

fn pin(model: &str, kind: Kind) {
    if let Ok(mut m) = cache().lock() {
        m.insert(model.to_string(), kind);
        save_pins(&m);
    }
}

/// One session id per process: the relay groups a conversation by it, and a
/// fresh one per request looks like a new client every time.
pub fn session() -> &'static str {
    static SESSION: OnceLock<String> = OnceLock::new();
    SESSION.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{nanos:x}-{:x}", std::process::id())
    })
}

/// Anonymous headers: the relay rejects unknown bearers, so send none, but it
/// does want to know which session this is.
pub fn extra_headers() -> Vec<(String, String)> {
    vec![
        (SESSION_HEADER.into(), session().to_string()),
        ("HTTP-Referer".into(), REFERER.into()),
        ("X-Title".into(), "Harness".into()),
    ]
}

pub fn headers() -> Vec<(String, String)> {
    let mut h = base_headers();
    h.extend(extra_headers());
    h
}

/// Which wire this model answers on, probing only when it is not known yet.
pub fn resolve(provider: &Provider, model: &str) -> Kind {
    if let Some(kind) = pinned(model) {
        return kind;
    }
    let kind = probe(provider, model).unwrap_or(Kind::OpenAi);
    pin(model, kind);
    kind
}

fn probe(provider: &Provider, model: &str) -> Option<Kind> {
    let base = provider.base_url.trim_end_matches('/');
    let bare = base.strip_suffix("/v1").unwrap_or(base);
    let payload = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "reply with the single word ok" }],
        "max_tokens": 16,
        "stream": false,
    })
    .to_string();
    if ok(&format!("{base}/chat/completions"), &payload) {
        return Some(Kind::OpenAi);
    }
    let anthropic = json!({
        "model": model,
        "max_tokens": 16,
        "messages": [{ "role": "user", "content": "reply with the single word ok" }],
    })
    .to_string();
    if ok(&format!("{bare}/v1/messages"), &anthropic) {
        return Some(Kind::Anthropic);
    }
    let responses = json!({
        "model": model,
        "input": "reply with the single word ok",
        "max_output_tokens": 16,
        "stream": false,
    })
    .to_string();
    if ok(&format!("{base}/responses"), &responses) {
        return Some(Kind::Responses);
    }
    None
}

/// A probe only wins on a plain 2xx that is not an Anthropic-shaped error.
fn ok(url: &str, payload: &str) -> bool {
    let mut headers = headers();
    headers.retain(|(k, _)| k != "Accept");
    headers.push(("Accept".into(), "application/json".into()));
    let req = HttpReq { url: url.to_string(), headers, body: payload.to_string() };
    let Ok(rx) = post(req, super::http::Cancel::new()) else { return false };
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut body = String::new();
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(HttpMsg::Line(line)) => {
                body.push_str(&line);
                if body.len() > 8192 {
                    break;
                }
            }
            Ok(HttpMsg::End { code, .. }) => {
                let ok = code.is_some_and(|c| (200..300).contains(&c));
                let looks_anthropic = body.contains("\"type\":\"error\"");
                return ok && !looks_anthropic;
            }
            Err(_) => break,
        }
    }
    false
}

/// The wire this provider should use, with the base URL put right for it.
pub fn route(provider: &Provider, model: &str) -> (Kind, Provider) {
    if provider.kind != Kind::Relay {
        return (provider.kind, provider.clone());
    }
    let kind = resolve(provider, model);
    let mut routed = provider.clone();
    routed.kind = kind;
    routed.api_key = KEYLESS.into();
    if kind == Kind::Anthropic {
        // This provider is keyless: drop the bearer before it goes out.
        let base = routed.base_url.trim_end_matches('/').trim_end_matches("/v1").to_string();
        routed.base_url = base;
    }
    // The relay rejects unknown bearers: anonymous is the only way in.
    if kind != Kind::Relay {
        routed.api_key = String::new();
    }
    (kind, routed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_round_trip_through_the_cache_shape() {
        let mut map = BTreeMap::new();
        map.insert("m".to_string(), Kind::Anthropic);
        let value: Value = map
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.as_str().to_string())))
            .collect::<serde_json::Map<_, _>>()
            .into();
        assert_eq!(value["m"], json!("anthropic"));
        assert_eq!(Kind::parse(value["m"].as_str().unwrap()), Some(Kind::Anthropic));
    }

    #[test]
    fn session_ids_are_stable_within_a_process() {
        assert_eq!(session(), session());
        assert!(!session().is_empty());
    }

    #[test]
    fn headers_are_anonymous_but_identify_the_client() {
        let h = headers();
        assert!(h.iter().any(|(k, _)| k.eq_ignore_ascii_case(SESSION_HEADER)));
        assert!(!h.iter().any(|(k, _)| k.eq_ignore_ascii_case("authorization")));
    }

    #[test]
    fn non_relay_providers_route_to_themselves() {
        let p = Provider {
            name: "openai".into(),
            kind: Kind::OpenAi,
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk".into(),
            models: Vec::new(),
        };
        let (kind, routed) = route(&p, "gpt-5");
        assert_eq!(kind, Kind::OpenAi);
        assert_eq!(routed.api_key, "sk");
    }
}
