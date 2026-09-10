//! The network: fetch a page, search the web, or make a raw request.
//!
//! Everything goes through curl, so proxies and certificates behave the way
//! the user's environment already has them set up. Pages come back as text,
//! not HTML, because a transcript is no place for markup.

use super::shell::run_capture;
use super::{Ctx, Tool, int_arg, str_arg, str_req};
use crate::llm::{number_prop, schema, string_prop};
use serde_json::{Value, json};

pub fn tools() -> Vec<Tool> {
    vec![
    Tool {
        name: "web_fetch",
        desc: "Fetch a URL and return it as readable text.",
        params: schema(
            json!({
                "url": string_prop("http or https URL"),
                "max_chars": number_prop("Truncate to this many characters"),
                "raw": string_prop("Set to 'html' to keep the markup"),
            }),
            &["url"],
        ),
        run: web_fetch,
        read_only: true,
    },
    Tool {
        name: "web_search",
        desc: "Search the web and return the top results with snippets.",
        params: schema(
            json!({
                "query": string_prop("Search terms"),
                "max": number_prop("How many results, default 8"),
            }),
            &["query"],
        ),
        run: web_search,
        read_only: true,
    },
    Tool {
        name: "http_request",
        desc: "Make an HTTP request with custom method, headers, and body.",
        params: schema(
            json!({
                "url": string_prop("URL"),
                "method": string_prop("GET, POST, PUT, DELETE, default GET"),
                "headers": json!({
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Header lines, 'Name: value'",
                }),
                "body": string_prop("Request body"),
            }),
            &["url"],
        ),
        run: http_request,
        read_only: false,
    },
    ]
}

fn curl(ctx: &mut Ctx, args: &str, timeout: u64) -> Result<String, String> {
    // curl with a phone-friendly identity and a hard cap on time.
    let cmd = format!(
        "curl -sS -L --max-time {} -A 'AndroidHarness/{}' {args}",
        timeout / 1000,
        env!("CARGO_PKG_VERSION")
    );
    run_capture(ctx, &cmd, timeout + 5_000)
}

fn web_fetch(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let url = str_req(args, "url")?;
    let max = int_arg(args, "max_chars").unwrap_or(40_000).clamp(500, 200_000) as usize;
    let raw = str_arg(args, "raw").unwrap_or_default() == "html";
    let body = curl(ctx, &format!("{}", quote(&url)), 45_000)?;
    if body.trim().is_empty() {
        return Err(format!("{url} returned nothing"));
    }
    let text = if raw { body } else { html_to_text(&body) };
    let mut out = text.trim().to_string();
    if out.len() > max {
        let mut end = max;
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push_str("\n… truncated");
    }
    Ok(out)
}

fn web_search(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let query = str_req(args, "query")?;
    let max = int_arg(args, "max").unwrap_or(8).clamp(1, 20);
    // DuckDuckGo's HTML endpoint: no key, no tracking script, parseable.
    let cmd = format!(
        "curl -sS -L --max-time 30 -A 'Mozilla/5.0 (AndroidHarness)' \
         --data-urlencode {} https://html.duckduckgo.com/html/",
        quote(&format!("q={query}"))
    );
    let body = run_capture(ctx, &cmd, 45_000)?;
    let results = parse_ddg(&body, max as usize);
    if results.is_empty() {
        return Err(format!("no results for '{query}' (the search endpoint may be blocked)"));
    }
    let mut out = format!("{query}\n\n");
    for (i, (title, url, snippet)) in results.iter().enumerate() {
        out.push_str(&format!("{}. {}\n   {}\n   {}\n\n", i + 1, title, url, snippet));
    }
    Ok(out.trim_end().to_string())
}

fn http_request(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let url = str_req(args, "url")?;
    let method = str_arg(args, "method").unwrap_or_else(|| "GET".into()).to_uppercase();
    let mut cmd = format!("-X {method}");
    if let Some(headers) = args.get("headers").and_then(|h| h.as_array()) {
        for h in headers.iter().filter_map(|h| h.as_str()) {
            cmd.push_str(&format!(" -H {}", quote(h)));
        }
    }
    if let Some(body) = str_arg(args, "body") {
        cmd.push_str(&format!(" --data-binary {}", quote(&body)));
    }
    let out = curl(ctx, &format!("{cmd} {}", quote(&url)), 60_000)?;
    Ok(super::clip(out))
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// -- html ------------------------------------------------------------------

/// Tags out, text in. Scripts and styles go entirely; entities get decoded so
/// the model does not read `&amp;` as five characters.
pub fn html_to_text(html: &str) -> String {
    let html = drop_blocks(html);
    let mut out = String::with_capacity(html.len() / 2);
    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut tag = String::new();
            for c in chars.by_ref() {
                if c == '>' {
                    break;
                }
                tag.push(c);
            }
            let name = tag
                .trim_start_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            match name.as_str() {
                "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "section"
                | "article" => out.push('\n'),
                _ => {}
            }
            continue;
        }
        if c == '&' {
            let mut entity = String::new();
            for c in chars.by_ref() {
                if c == ';' || entity.len() > 8 {
                    break;
                }
                entity.push(c);
            }
            out.push_str(&decode_entity(&entity));
            continue;
        }
        out.push(c);
    }
    collapse_blank_lines(&out).trim_end().to_string()
}

/// Remove whole elements whose contents are never useful as prose. Done by
/// substring rather than by tag walking, because a `<` inside a script would
/// otherwise look like the start of the next tag.
fn drop_blocks(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < html.len() {
        let mut cut: Option<(usize, usize)> = None;
        for tag in ["script", "style", "noscript", "svg"] {
            let open = format!("<{tag}");
            let Some(pos) = lower[i..].find(&open) else { continue };
            let start = i + pos;
            let close = format!("</{tag}");
            let end = lower[start..]
                .find(&close)
                .map(|p| start + p + close.len())
                .unwrap_or(html.len());
            let end = lower[end..].find('>').map(|p| end + p + 1).unwrap_or(html.len());
            if cut.map(|(s, _)| start < s).unwrap_or(true) {
                cut = Some((start, end));
            }
        }
        match cut {
            Some((start, end)) => {
                out.push_str(&html[i..start]);
                out.push(' ');
                i = end.max(start);
            }
            None => {
                out.push_str(&html[i..]);
                break;
            }
        }
    }
    out
}

fn decode_entity(entity: &str) -> String {
    match entity {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" | "#39" | "#x27" => "'".into(),
        "nbsp" => " ".into(),
        "mdash" => "—".into(),
        "ndash" => "–".into(),
        "hellip" => "…".into(),
        other => {
            let code = other
                .strip_prefix("#x")
                .or_else(|| other.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| other.strip_prefix('#').and_then(|d| d.parse().ok()));
            match code.and_then(char::from_u32) {
                Some(c) => c.to_string(),
                None => format!("&{entity};"),
            }
        }
    }
}

fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::new();
    let mut blanks = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    out
}

/// Pull title, url, and snippet out of a DuckDuckGo results page.
pub fn parse_ddg(html: &str, max: usize) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut rest = html;
    while out.len() < max {
        let Some(start) = rest.find("result__a") else { break };
        rest = &rest[start..];
        let Some(href) = rest.find("href=\"") else { break };
        let after = &rest[href + 6..];
        let Some(end) = after.find('"') else { break };
        let url = clean_url(&after[..end]);
        let Some(gt) = after.find('>') else { break };
        let tail = &after[gt + 1..];
        let Some(close) = tail.find("</a>") else { break };
        let title = html_to_text(&tail[..close]).trim().to_string();
        let snippet = tail[close..]
            .find("result__snippet")
            .map(|s| {
                let chunk = &tail[close + s..];
                let text = chunk.split("</a>").next().unwrap_or(chunk);
                html_to_text(text).trim().to_string()
            })
            .unwrap_or_default();
        let snippet = snippet.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join(" ");
        if !url.is_empty() && !title.is_empty() {
            out.push((title, url, snippet.chars().take(300).collect()));
        }
        rest = &tail[close..];
    }
    out
}

/// DuckDuckGo wraps links: /l/?uddg=<encoded>.
fn clean_url(raw: &str) -> String {
    let decoded = percent_decode(raw);
    match decoded.find("uddg=") {
        Some(at) => {
            let rest = &decoded[at + 5..];
            let end = rest.find('&').unwrap_or(rest.len());
            percent_decode(&rest[..end])
        }
        None => decoded,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_readable_text() {
        let html = "<html><head><style>p{color:red}</style><title>T</title></head><body>\
                    <h1>Hello &amp; welcome</h1><p>First <b>para</b>.</p>\
                    <script>var x = 1 < 2;</script><p>Second</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Hello & welcome"), "{text}");
        assert!(text.contains("First para."), "{text}");
        assert!(text.contains("Second"));
        assert!(!text.contains("var x"), "scripts are dropped");
        assert!(!text.contains("color:red"));
    }

    #[test]
    fn entities_decode_including_numeric() {
        assert_eq!(html_to_text("a&#65;b"), "aAb");
        assert_eq!(html_to_text("x&#x41;"), "xA");
        assert_eq!(html_to_text("&unknown;"), "&unknown;");
    }

    #[test]
    fn blank_runs_collapse() {
        assert_eq!(html_to_text("a<br><br><br><br>b"), "a\n\nb");
    }

    #[test]
    fn a_less_than_inside_a_script_does_not_eat_the_page() {
        let html = "<p>one</p><script>if (a < b) { c(); }</script><p>two</p>";
        let text = html_to_text(html);
        assert!(text.contains("one"), "{text}");
        assert!(text.contains("two"), "{text}");
        assert!(!text.contains("c()"), "{text}");
    }

    #[test]
    fn ddg_results_parse() {
        let html = r#"<div class="result"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1&amp;rut=zz">Example <b>Site</b></a>
        <a class="result__snippet">A snippet &amp; more</a></div>"#;
        let out = parse_ddg(html, 5);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "Example Site");
        assert_eq!(out[0].1, "https://example.com/a?b=1");
        assert!(out[0].2.contains("A snippet & more"), "{}", out[0].2);
    }

    #[test]
    fn percent_decoding_is_lenient() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
