//! Asking a provider what models it serves.
//!
//! The picker opens immediately with whatever the config already knows, then
//! this fills in the live list when the request comes back. Providers that do
//! not answer just leave the configured list in place.

use super::http::{self, Cancel};
use crate::config::{Kind, Provider};

/// Model ids for this provider, newest list first. Blocking, with a cap.
pub fn list(provider: &Provider) -> Result<Vec<String>, String> {
    let cancel = Cancel::new();
    let base = provider.base_url.trim_end_matches('/');
    let (url, headers) = match provider.kind {
        Kind::Gemini => (
            format!("{base}/models"),
            vec![("x-goog-api-key".into(), provider.api_key.clone())],
        ),
        Kind::Anthropic => (
            format!("{base}/v1/models?limit=100"),
            vec![
                ("x-api-key".into(), provider.api_key.clone()),
                ("anthropic-version".into(), super::anthropic::VERSION.into()),
            ],
        ),
        Kind::Relay => (
            format!("{base}/models"),
            super::relay::extra_headers(),
        ),
        _ => {
            let mut headers = Vec::new();
            if !provider.api_key.is_empty() {
                headers.push(("Authorization".into(), format!("Bearer {}", provider.api_key)));
            }
            (format!("{base}/models"), headers)
        }
    };
    let body = http::get(&url, &headers, cancel, 25_000)?;
    let mut ids = parse(&body);
    if provider.kind == Kind::Relay {
        // The relay lists its whole catalog, but the keyless tier only serves
        // the free slots, so offering the rest would be offering failures.
        ids.retain(|id| free_slot(id) || provider.models.iter().any(|m| m == id));
    }
    if ids.is_empty() {
        return Err(format!("{url} returned no models"));
    }
    Ok(ids)
}

/// Free slots on the keyless relay.
pub fn free_slot(id: &str) -> bool {
    id.ends_with("-free") || id == "big-pickle"
}

/// Every shape of a model list we have met.
pub fn parse(body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    // OpenAI style: {"data":[{"id":"gpt-5"}]}
    if let Some(items) = v.get("data").and_then(|d| d.as_array()) {
        for item in items {
            if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                out.push(id.to_string());
            }
        }
    }
    // Gemini style: {"models":[{"name":"models/gemini-2.5-pro"}]}
    if let Some(items) = v.get("models").and_then(|m| m.as_array()) {
        for item in items {
            let name = item
                .get("id")
                .and_then(|i| i.as_str())
                .or_else(|| item.get("name").and_then(|n| n.as_str()));
            if let Some(name) = name {
                out.push(name.trim_start_matches("models/").to_string());
            }
        }
    }
    // A bare array of strings, seen from a few gateways.
    if let Some(items) = v.as_array() {
        for item in items {
            if let Some(name) = item.as_str() {
                out.push(name.to_string());
            }
        }
    }
    out.retain(|id| !id.is_empty());
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Kind;

    #[test]
    fn openai_shape() {
        let ids = parse(r#"{"object":"list","data":[{"id":"gpt-5"},{"id":"gpt-4o-mini"}]}"#);
        assert_eq!(ids, vec!["gpt-4o-mini", "gpt-5"]);
    }

    #[test]
    fn gemini_shape_strips_the_prefix() {
        let ids = parse(r#"{"models":[{"name":"models/gemini-2.5-pro"},{"name":"models/gemini-2.5-flash"}]}"#);
        assert_eq!(ids, vec!["gemini-2.5-flash", "gemini-2.5-pro"]);
    }

    #[test]
    fn bare_array_and_duplicates() {
        let ids = parse(r#"["a","b","a"]"#);
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn junk_is_not_a_crash() {
        assert!(parse("<html>nope</html>").is_empty());
        assert!(parse("{}").is_empty());
    }

    #[test]
    fn free_slots_are_recognized() {
        assert!(free_slot("ling-3.0-flash-fin-free"));
        assert!(free_slot("big-pickle"));
        assert!(!free_slot("claude-opus-5"));
        let mut ids = vec!["claude-opus-5".to_string(), "mimo-v2.5-free".to_string()];
        ids.retain(|id| free_slot(id) || id == "claude-opus-5");
        assert_eq!(ids.len(), 2, "a model already in the config stays listed");
    }

    #[test]
    fn a_provider_without_a_key_still_builds_a_request() {
        let p = Provider {
            name: "local".into(),
            kind: Kind::OpenAi,
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: String::new(),
            models: Vec::new(),
        };
        // Port 1 refuses instantly: the point is that it fails rather than
        // panicking or hanging.
        assert!(list(&p).is_err());
    }

    /// Live check against the keyless relay. Run with:
    ///   cargo test -- --ignored --nocapture live_relay
    #[test]
    #[ignore = "hits the network"]
    fn live_relay_lists_free_models() {
        let p = Provider {
            name: "harness".into(),
            kind: Kind::Relay,
            base_url: crate::config::HARNESS_BASE.into(),
            api_key: String::new(),
            models: Vec::new(),
        };
        let ids = list(&p).expect("the relay should answer");
        println!("{} free models", ids.len());
        assert!(ids.iter().all(|id| free_slot(id)), "only free slots are offered: {ids:?}");
        assert!(ids.len() > 3, "{ids:?}");
    }
}
