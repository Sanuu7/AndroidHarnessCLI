//! What a session cost.
//!
//! A small built-in table, deliberately conservative: a model that is not in
//! it reports no price at all rather than a made-up number, and the status bar
//! shows that as a dash.

use crate::llm::Usage;

/// (input, output, cached input) in US dollars per million tokens.
fn table(model: &str) -> Option<(f64, f64, f64)> {
    let m = model.to_ascii_lowercase();
    if m.ends_with("-free") || m.contains(":free") || m == "big-pickle" {
        return Some((0.0, 0.0, 0.0));
    }
    let known: &[(&str, f64, f64, f64)] = &[
        ("claude-opus-4", 15.0, 75.0, 1.5),
        ("claude-sonnet-4", 3.0, 15.0, 0.3),
        ("claude-haiku-4", 1.0, 5.0, 0.1),
        ("claude-3-5-haiku", 0.8, 4.0, 0.08),
        ("gemini-2.5-pro", 1.25, 10.0, 0.31),
        ("gemini-2.5-flash", 0.3, 2.5, 0.075),
        ("gemini-2.0-flash", 0.1, 0.4, 0.025),
        ("gpt-5", 1.25, 10.0, 0.125),
        ("gpt-4.1-mini", 0.4, 1.6, 0.1),
        ("gpt-4.1", 2.0, 8.0, 0.5),
        ("gpt-4o-mini", 0.15, 0.6, 0.075),
        ("gpt-4o", 2.5, 10.0, 1.25),
        ("o3", 2.0, 8.0, 0.5),
        ("o4-mini", 1.1, 4.4, 0.275),
        ("deepseek-chat", 0.27, 1.1, 0.07),
        ("deepseek-reasoner", 0.55, 2.19, 0.14),
        ("glm-4", 0.6, 2.2, 0.11),
        ("glm-5", 0.6, 2.2, 0.11),
        ("qwen", 0.4, 1.2, 0.1),
        ("kimi", 0.6, 2.5, 0.15),
        ("minimax", 0.3, 1.2, 0.03),
        ("mistral-large", 2.0, 6.0, 0.5),
        ("llama", 0.2, 0.6, 0.05),
    ];
    known
        .iter()
        .find(|(prefix, _, _, _)| m.starts_with(prefix) || m.contains(prefix))
        .map(|(_, i, o, c)| (*i, *o, *c))
}

/// Dollars for one exchange. `None` when the model is not priced here.
pub fn estimate(model: &str, usage: &Usage) -> Option<f64> {
    let (input, output, cached) = table(model)?;
    let fresh = usage.input.saturating_sub(usage.cache_read);
    let cost = fresh as f64 * input / 1e6
        + usage.cache_read as f64 * cached / 1e6
        + usage.cache_write as f64 * input * 1.25 / 1e6
        + usage.output as f64 * output / 1e6;
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64) -> Usage {
        Usage { input, output, cache_read: 0, cache_write: 0 }
    }

    #[test]
    fn free_models_cost_nothing() {
        let u = usage(1_000_000, 1_000_000);
        assert_eq!(estimate("ling-3.0-flash-fin-free", &u), Some(0.0));
        assert_eq!(estimate("mimo-v2.5-free", &u), Some(0.0));
        assert_eq!(estimate("big-pickle", &u), Some(0.0));
    }

    #[test]
    fn known_models_price_by_token() {
        let cost = estimate("claude-sonnet-4-5", &usage(1_000_000, 1_000_000)).unwrap();
        assert!((cost - 18.0).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn cached_input_is_cheaper() {
        let plain = estimate("claude-sonnet-4-5", &usage(1_000_000, 0)).unwrap();
        let cached = estimate(
            "claude-sonnet-4-5",
            &Usage { input: 1_000_000, output: 0, cache_read: 1_000_000, cache_write: 0 },
        )
        .unwrap();
        assert!(cached < plain);
        assert!((cached - 0.3).abs() < 1e-9, "{cached}");
    }

    #[test]
    fn unknown_models_report_no_price() {
        assert_eq!(estimate("someone-private-model", &usage(1_000, 1_000)), None);
    }

    #[test]
    fn long_names_match_by_substring() {
        assert!(estimate("openai/gpt-4o-mini-2024-07-18", &usage(1_000, 0)).is_some());
        assert!(estimate("anthropic/claude-sonnet-4.5", &usage(1_000, 0)).is_some());
    }
}
