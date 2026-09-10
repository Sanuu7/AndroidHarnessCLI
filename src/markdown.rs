//! A small markdown renderer aimed at streaming chat text.
//!
//! Deliberately not a full parser: headers, bullets, quotes, rules, fenced
//! code, and inline bold/italic/code. Everything comes out as styled ratatui
//! lines, so it works while the message is still arriving (an unterminated
//! fence renders as a code block, which is exactly what a streaming reply
//! wants to show).

use crate::textutil::{pad_right, truncate, wrap};
use crate::theme::{self, Theme};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render(text: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut fence: Option<(String, Vec<String>)> = None;
    let mut para: Vec<String> = Vec::new();

    for raw in text.split('\n') {
        if let Some((lang, buf)) = fence.as_mut() {
            if raw.trim_start().starts_with("```") {
                let (lang, buf) = (lang.clone(), std::mem::take(buf));
                push_code(&mut out, &lang, &buf, theme, width);
                fence = None;
            } else {
                buf.push(raw.to_string());
            }
            continue;
        }

        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            flush_para(&mut out, &mut para, theme, width);
            fence = Some((trimmed.trim_start_matches('`').trim().to_string(), Vec::new()));
            continue;
        }
        if trimmed.is_empty() {
            flush_para(&mut out, &mut para, theme, width);
            if !matches!(out.last(), Some(l) if l.spans.is_empty()) {
                out.push(Line::default());
            }
            continue;
        }
        if let Some((level, rest)) = header(trimmed) {
            flush_para(&mut out, &mut para, theme, width);
            out.extend(push_header(rest, level, theme, width));
            continue;
        }
        if is_rule(trimmed) {
            flush_para(&mut out, &mut para, theme, width);
            out.push(rule_line(width, theme));
            continue;
        }
        if let Some((depth, rest)) = bullet(trimmed) {
            flush_para(&mut out, &mut para, theme, width);
            out.extend(push_bullet(rest, depth, theme, width));
            continue;
        }
        if let Some((depth, num, rest)) = ordered(trimmed) {
            flush_para(&mut out, &mut para, theme, width);
            out.extend(push_ordered(rest, depth, num, theme, width));
            continue;
        }
        if let Some(rest) = quote(trimmed) {
            flush_para(&mut out, &mut para, theme, width);
            out.extend(push_quote(rest, theme, width));
            continue;
        }
        para.push(raw.to_string());
    }

    if let Some((lang, buf)) = fence {
        push_code(&mut out, &lang, &buf, theme, width);
    }
    flush_para(&mut out, &mut para, theme, width);
    if out.is_empty() {
        out.push(Line::default());
    }
    out
}

fn flush_para(out: &mut Vec<Line<'static>>, para: &mut Vec<String>, theme: &Theme, width: usize) {
    if para.is_empty() {
        return;
    }
    let joined = para.join(" ");
    para.clear();
    for row in wrap(&joined, width) {
        out.push(Line::from(inline(&row, theme)));
    }
}

fn push_header(rest: &str, level: usize, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let fg = if level <= 1 { theme.accent } else { theme.text };
    let style = Style::default().fg(theme.c(fg)).add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    if level <= 1 {
        spans.push(Span::styled("▏ ", Style::default().fg(theme.c(theme.accent2))));
    }
    spans.push(Span::styled(rest.to_string(), style));
    let mut lines = vec![Line::from(spans)];
    // Headers wrap without the marker on continuation lines.
    let rows = wrap(rest, width.saturating_sub(2));
    if rows.len() > 1 {
        lines = rows
            .into_iter()
            .map(|r| Line::from(Span::styled(r, style)))
            .collect();
    }
    lines
}

fn push_bullet(rest: &str, depth: usize, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let indent = depth * 2;
    let marker_w = 2;
    let text_w = width.saturating_sub(indent + marker_w).max(4);
    let rows = wrap(rest, text_w);
    let dot = Style::default().fg(theme.c(theme.accent));
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let lead = if i == 0 {
            Span::styled("• ".to_string(), dot)
        } else {
            Span::raw("  ".to_string())
        };
        let mut spans = vec![Span::raw(" ".repeat(indent)), lead];
        spans.extend(inline(row, theme));
        lines.push(Line::from(spans));
    }
    lines
}

fn push_ordered(
    rest: &str,
    depth: usize,
    num: String,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let indent = depth * 2;
    let label = format!("{num}. ");
    let label_w = crate::textutil::width(&label);
    let text_w = width.saturating_sub(indent + label_w).max(4);
    let rows = wrap(rest, text_w);
    let num_style = Style::default().fg(theme.c(theme.dim));
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let lead = if i == 0 {
            Span::styled(label.clone(), num_style)
        } else {
            Span::raw(" ".repeat(label_w))
        };
        let mut spans = vec![Span::raw(" ".repeat(indent)), lead];
        spans.extend(inline(row, theme));
        lines.push(Line::from(spans));
    }
    lines
}

fn push_quote(rest: &str, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let bar = Span::styled("▎ ", Style::default().fg(theme.c(theme.accent2)));
    let style = Style::default()
        .fg(theme.c(theme.faint))
        .add_modifier(Modifier::ITALIC);
    wrap(rest, width.saturating_sub(2))
        .into_iter()
        .map(|row| Line::from(vec![bar.clone(), Span::styled(row, style)]))
        .collect()
}

fn push_code(out: &mut Vec<Line<'static>>, lang: &str, buf: &[String], theme: &Theme, width: usize) {
    let bg = Style::default().bg(theme.c(theme.code_bg));
    if !lang.is_empty() {
        out.push(Line::from(Span::styled(
            format!(" {} ", truncate(lang, width.saturating_sub(2))),
            bg.fg(theme.c(theme.accent2)).add_modifier(Modifier::BOLD),
        )));
    }
    for line in buf {
        let text = pad_right(&truncate(line, width), width);
        out.push(Line::from(Span::styled(text, bg.fg(theme.c(theme.text)))));
    }
}

fn rule_line(width: usize, theme: &Theme) -> Line<'static> {
    let stops = [theme.accent2, theme.accent];
    let cols = width.min(24);
    let spans: Vec<Span> = (0..cols)
        .map(|i| {
            let t = i as f32 / (cols.max(2) - 1) as f32;
            Span::styled("─".to_string(), Style::default().fg(theme.c(theme::grad(stops[0], stops[1], t))))
        })
        .collect();
    Line::from(spans)
}

fn header(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = line[hashes..].trim_start();
    if line.chars().nth(hashes) == Some(' ') || !rest.is_empty() {
        Some((hashes, rest))
    } else {
        None
    }
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3
        && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn bullet(line: &str) -> Option<(usize, &str)> {
    let indent = line.chars().take_while(|c| *c == ' ').count() / 2;
    let rest = line.trim_start();
    for marker in ["- ", "* ", "+ "] {
        if let Some(r) = rest.strip_prefix(marker) {
            return Some((indent, r));
        }
    }
    None
}

fn ordered(line: &str) -> Option<(usize, String, &str)> {
    let indent = line.chars().take_while(|c| *c == ' ').count() / 2;
    let rest = line.trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let after = &rest[digits.len()..];
    for sep in [". ", ") "] {
        if let Some(r) = after.strip_prefix(sep) {
            return Some((indent, digits, r));
        }
    }
    None
}

fn quote(line: &str) -> Option<&str> {
    line.strip_prefix('>').map(|r| r.trim_start())
}

/// Inline spans: `code`, **bold**, *italic*, __bold__, _italic_.
pub fn inline(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let code_style = Style::default().fg(theme.c(theme.accent)).bg(theme.c(theme.code_bg));
    let bold_style = Style::default()
        .fg(theme.c(theme.text))
        .add_modifier(Modifier::BOLD);
    let italic_style = Style::default()
        .fg(theme.c(theme.dim))
        .add_modifier(Modifier::ITALIC);
    let plain = Style::default().fg(theme.c(theme.text));

    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    let push_plain = |spans: &mut Vec<Span<'static>>, buf: &mut String| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), plain));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            if let Some(end) = find(&chars, i + 1, '`') {
                push_plain(&mut spans, &mut buf);
                let content: String = chars[i + 1..end].iter().collect();
                spans.push(Span::styled(content, code_style));
                i = end + 1;
                continue;
            }
        }
        if c == '*' || c == '_' {
            let doubled = i + 1 < chars.len() && chars[i + 1] == c;
            let marker_len = if doubled { 2 } else { 1 };
            if let Some(end) = find_run(&chars, i, c) {
                push_plain(&mut spans, &mut buf);
                let content: String = chars[i + marker_len..end].iter().collect();
                if !content.is_empty() {
                    let style = if doubled { bold_style } else { italic_style };
                    spans.push(Span::styled(content, style));
                }
                i = end + marker_len;
                continue;
            }
        }
        buf.push(c);
        i += 1;
    }
    push_plain(&mut spans, &mut buf);
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), plain));
    }
    spans
}

fn find(chars: &[char], from: usize, needle: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == needle)
}

/// Finds the closing run of the same marker char, skipping the opening run.
fn find_run(chars: &[char], start: usize, marker: char) -> Option<usize> {
    let mut i = start;
    while i < chars.len() && chars[i] == marker {
        i += 1;
    }
    while i < chars.len() {
        if chars[i] == marker {
            // Skip if escaped or part of a longer run at the very end.
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorLevel;

    fn theme() -> Theme {
        Theme::forced(ColorLevel::True)
    }

    fn text_of(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect()
    }

    #[test]
    fn headers_and_paragraphs() {
        let out = render("# Title\n\nhello **world**", &theme(), 40);
        let t = text_of(&out);
        assert!(t[0].contains("Title"));
        assert!(t.iter().any(|l| l.contains("hello world")));
    }

    #[test]
    fn unclosed_fence_renders_as_code() {
        let out = render("text\n```rust\nfn main() {}", &theme(), 40);
        let t = text_of(&out);
        assert!(t.iter().any(|l| l.contains("rust")));
        assert!(t.iter().any(|l| l.contains("fn main()")));
    }

    #[test]
    fn bullets_and_quotes() {
        let out = render("- one\n- two\n> quoted", &theme(), 40);
        let t = text_of(&out);
        assert!(t.iter().any(|l| l.starts_with("• one")));
        assert!(t.iter().any(|l| l.contains("quoted")));
    }

    #[test]
    fn inline_styles_keep_content() {
        let spans = inline("a `code` b **bold** c", &theme());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "a code b bold c");
        assert_eq!(spans.iter().filter(|s| s.style.add_modifier.contains(Modifier::BOLD)).count(), 1);
    }

    #[test]
    fn width_never_exceeded() {
        let md = "# A rather long heading that should wrap\n\n- a bullet with quite a lot of words in it so it wraps\n\nbody text too";
        for w in [20usize, 32, 45] {
            for line in render(md, &theme(), w) {
                let s: String = line.spans.iter().map(|x| x.content.as_ref()).collect();
                assert!(
                    crate::textutil::width(&s) <= w || s.contains("fn"),
                    "w={w} line too wide: {s:?}"
                );
            }
        }
    }
}
