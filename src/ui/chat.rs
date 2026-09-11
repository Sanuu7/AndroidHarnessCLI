//! The transcript: messages, thinking, tool cards, and the welcome screen.
//!
//! Lines are rebuilt each frame from the item list. That is cheap at chat
//! scale and keeps every animation (fade in, comet tail, skeleton shimmer,
//! card settle, completion sweep) a pure function of `app.now`.

use crate::anim::{self, Tween};
use crate::app::{App, Item, ToolState};
use crate::markdown;
use crate::textutil::{truncate, width, wrap};
use crate::theme::{self, Theme};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Indent for a message body, so wrapped lines hang under the first one.
const USER_INDENT: usize = 5;
const BODY_INDENT: usize = 2;
const CARD_INDENT: usize = 4;
/// Expanded tool output stops here; a transcript is not a file viewer.
const MAX_CARD_LINES: usize = 40;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if app.is_empty_state() {
        welcome(frame, area, app);
        return;
    }
    // A column of air on each side: on a phone, text that touches the glass
    // is the difference between cramped and readable.
    let outer = area;
    let area = Rect {
        x: outer.x + 1,
        y: outer.y,
        width: outer.width.saturating_sub(2),
        height: outer.height,
    };
    if area.width < 12 {
        return;
    }
    let w = area.width as usize;
    let scale = app.appear(app.chat_at).max(0.15);
    let mut lines: Vec<Line> = Vec::new();
    // Where the last thing the user said starts, so the fade mask above never
    // dims the turn being read.
    let mut last_user_row = 0usize;
    let mut prev_is_tool = false;
    for item in app.items.iter() {
        let is_tool = matches!(item, Item::Tool { .. });
        if !lines.is_empty() && !(prev_is_tool && is_tool) {
            lines.push(Line::default());
        }
        prev_is_tool = is_tool;
        if matches!(item, Item::User { .. }) {
            last_user_row = lines.len();
        }
        lines.extend(item_lines(item, app, w, scale));
    }

    let total = lines.len();
    let h = area.height as usize;
    let max_scroll = total.saturating_sub(h);
    let scroll = app.scroll.min(max_scroll);
    let start = total.saturating_sub(h + scroll);
    let mut visible: Vec<Line> = lines.into_iter().skip(start).take(h).collect();

    // The top rows fade out when there is more above, but only above the
    // current turn.
    if start > 0 {
        let mask_rows = 2.min(visible.len());
        let fade_rows = last_user_row.saturating_sub(start).min(mask_rows);
        for (row, line) in visible.iter_mut().take(fade_rows).enumerate() {
            let keep = anim::top_mask(row, mask_rows, 0.85);
            *line = fade_line(line.clone(), app.theme.bg, keep);
        }
    }

    // Short transcripts sit on the composer instead of floating at the top,
    // the way a chat should read.
    let used = visible.len() as u16;
    let anchored = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(used),
        width: area.width,
        height: used,
    };
    frame.render_widget(Paragraph::new(ratatui::text::Text::from(visible)), anchored);

    if scroll > 0 {
        draw_scroll_hint(frame, outer, app, scroll);
    }
}

fn draw_scroll_hint(frame: &mut Frame, area: Rect, app: &App, scroll: usize) {
    let t = &app.theme;
    let bob = anim::breathe(app.now, 1400);
    let color = theme::lerp(t.faint, t.accent, bob * 0.6);
    let text = format!("↓ {scroll}");
    let x = area.right().saturating_sub(text.len() as u16 + 1);
    let y = area.bottom().saturating_sub(1);
    frame.buffer_mut().set_string(
        x,
        y,
        text,
        Style::default().fg(t.c(color)).add_modifier(Modifier::BOLD),
    );
}

// -- welcome ---------------------------------------------------------------

/// Shown before anything has been said. Same identity as the splash, quieter,
/// with three things to try.
fn welcome(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let cy = area.y + area.height / 3;

    let mut word: Vec<Span> = Vec::new();
    let band = anim::saw(app.now, 2600);
    let letters: Vec<char> = "harness".chars().collect();
    let n = letters.len();
    for (i, ch) in letters.iter().enumerate() {
        let base = theme::grad(t.accent, t.accent2, i as f32 / (n - 1) as f32);
        let lit = theme::lerp(base, (255, 255, 255), anim::bell(i as f32, band * n as f32, 1.2) * 0.4);
        // Letters arrive one after another the first time it appears.
        let appear = Tween::delayed(i as u64 * 60, 320, anim::Ease::OutCubic).t(app.now, app.chat_at);
        word.push(Span::styled(
            ch.to_string(),
            Style::default()
                .fg(t.fade(lit, appear))
                .add_modifier(Modifier::BOLD),
        ));
    }

    let diamond_pulse = anim::breathe(app.now, 2400);
    let diamond = Span::styled(
        "◆".to_string(),
        Style::default().fg(t.fade(theme::lerp(t.accent, (255, 255, 255), diamond_pulse * 0.5), 1.0)),
    );

    let hints = [
        "ask about this repo",
        "/doctor runs the self test",
        "/help lists the keys",
    ];
    let mut lines: Vec<Line> = vec![
        Line::from(diamond).alignment(Alignment::Center),
        Line::from(word).alignment(Alignment::Center),
        Line::default(),
    ];
    for (i, hint) in hints.iter().enumerate() {
        let appear = Tween::delayed(400 + i as u64 * 140, 380, anim::Ease::OutCubic)
            .t(app.now, app.chat_at);
        lines.push(Line::from(vec![
            Span::styled("· ", Style::default().fg(t.fade(t.accent2, appear * 0.8))),
            Span::styled(hint.to_string(), Style::default().fg(t.fade(t.faint, appear))),
        ])
        .alignment(Alignment::Center));
    }

    let block_h = lines.len() as u16;
    let y = cy.saturating_sub(block_h / 2).max(area.y);
    let rect = Rect {
        x: area.x + 2,
        y,
        width: area.width.saturating_sub(4),
        height: block_h.min(area.bottom().saturating_sub(y)),
    };
    frame.render_widget(Paragraph::new(ratatui::text::Text::from(lines)), rect);
}

// -- items -----------------------------------------------------------------

fn item_lines(item: &Item, app: &App, w: usize, scale: f32) -> Vec<Line<'static>> {
    let t = &app.theme;
    let alpha = app.appear(item.born()) * scale;
    match item {
        Item::User { text, .. } => user_lines(text, t, w, alpha),
        Item::Assistant {
            text,
            streaming,
            finished,
            ..
        } => assistant_lines(text, *streaming, *finished, app, w, alpha),
        Item::Thinking { born } => thinking_lines(app, *born, w, alpha),
        Item::Reasoning { text, born, finished } => {
            reasoning_lines(app, text, *born, *finished, w, alpha)
        }
        Item::Tool {
            name,
            args,
            state,
            expanded,
            expand_at,
            ..
        } => tool_lines(app, name, args, state, *expanded, *expand_at, w, alpha),
        Item::Note {
            text,
            error,
            born,
        } => note_lines(app, text, *error, *born, alpha, w),
    }
}

/// Reasoning is not the answer: while it streams it shows the tail with a
/// spinner header, and once the answer starts it collapses to one line that
/// says how long it thought. ctrl+t opens it again (pi's thinking toggle,
/// Reasoning is not the answer: while it streams it shows a single quiet line
/// with a spinner, and once finished it shows how long it thought.
/// ctrl+t opens it to read the thought process in clean italic dim text.
fn reasoning_lines(
    app: &App,
    text: &str,
    born: u64,
    finished: u64,
    w: usize,
    alpha: f32,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let ink = theme::thinking_color(app.status.thinking);
    let open = app.thinking_open || app.expand;

    if !open {
        let mut spans = Vec::new();
        if finished > 0 {
            let secs = (finished.saturating_sub(born)) as f32 / 1000.0;
            spans.push(Span::styled(
                "· ".to_string(),
                Style::default().fg(t.fade(ink, alpha * 0.6)),
            ));
            spans.push(Span::styled(
                if secs >= 1.0 {
                    format!("Thought for {secs:.0}s")
                } else {
                    "Thought".to_string()
                },
                Style::default()
                    .fg(t.fade(ink, alpha * 0.9))
                    .add_modifier(Modifier::ITALIC),
            ));
        } else {
            let secs = (app.now.saturating_sub(born)) as f32 / 1000.0;
            spans.push(Span::styled(
                format!("{} ", anim::spinner(app.now)),
                Style::default().fg(t.fade(t.amber, alpha)),
            ));
            spans.push(Span::styled(
                "Thinking...".to_string(),
                Style::default()
                    .fg(t.fade(ink, alpha))
                    .add_modifier(Modifier::ITALIC),
            ));
            if secs >= 1.0 {
                spans.push(Span::styled(
                    format!(" ({secs:.1}s)"),
                    Style::default().fg(t.fade(t.faint, alpha * 0.8)),
                ));
            }
        }
        if !app.status.thinking.is_off() && w > 36 {
            spans.push(Span::styled(
                format!(" · {}", app.status.thinking.as_str()),
                Style::default().fg(t.fade(t.faint, alpha * 0.7)),
            ));
        }
        return vec![Line::from(spans)];
    }

    // Expanded view (toggled via ctrl+t)
    let mut out: Vec<Line<'static>> = Vec::new();
    let secs = if finished > 0 {
        (finished.saturating_sub(born)) as f32 / 1000.0
    } else {
        (app.now.saturating_sub(born)) as f32 / 1000.0
    };
    let mut header_spans = vec![
        Span::styled(
            if finished > 0 { "▾ " } else { "⠋ " },
            Style::default().fg(t.fade(if finished > 0 { ink } else { t.amber }, alpha)),
        ),
        Span::styled(
            if finished > 0 {
                if secs >= 1.0 {
                    format!("Thought for {secs:.0}s")
                } else {
                    "Thought".to_string()
                }
            } else {
                "Thinking...".to_string()
            },
            Style::default()
                .fg(t.fade(ink, alpha))
                .add_modifier(Modifier::ITALIC),
        ),
    ];
    if finished == 0 && secs >= 1.0 {
        header_spans.push(Span::styled(
            format!(" ({secs:.1}s)"),
            Style::default().fg(t.fade(t.faint, alpha * 0.8)),
        ));
    }
    if !app.status.thinking.is_off() && w > 36 {
        header_spans.push(Span::styled(
            format!(" · {}", app.status.thinking.as_str()),
            Style::default().fg(t.fade(t.faint, alpha * 0.7)),
        ));
    }
    out.push(Line::from(header_spans));

    let indent = BODY_INDENT;
    let text_w = w.saturating_sub(indent).max(8);
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            out.push(Line::default());
            continue;
        }
        for row in wrap(trimmed, text_w) {
            out.push(Line::from(vec![
                Span::raw(" ".repeat(indent)),
                Span::styled(
                    row,
                    Style::default()
                        .fg(t.fade(ink, alpha * 0.85))
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
    }
    out
}

/// The user's own words sit on a tinted block, so a long transcript has
/// anchors to scroll by (pi gives user messages their own background).
fn user_lines(text: &str, t: &Theme, w: usize, alpha: f32) -> Vec<Line<'static>> {
    let body_w = w.saturating_sub(USER_INDENT).max(8);
    let fill = t.c(theme::lerp(t.bg, t.user_bg, 0.75 * alpha));
    let head = vec![
        Span::styled("▌".to_string(), Style::default().fg(t.fade(t.accent, alpha))),
        Span::styled(
            "you".to_string(),
            Style::default()
                .fg(t.fade(t.dim, alpha))
                .add_modifier(Modifier::BOLD)
                .bg(fill),
        ),
        Span::styled(" ".to_string(), Style::default().bg(fill)),
    ];
    let mut out = Vec::new();
    for (i, row) in wrap(text, body_w).into_iter().enumerate() {
        let mut spans = if i == 0 {
            head.clone()
        } else {
            vec![
                Span::styled(" ".repeat(USER_INDENT), Style::default().bg(fill)),
            ]
        };
        // Pad the row so the tint reaches the edge it started at.
        let pad = body_w.saturating_sub(width(&row));
        spans.push(Span::styled(
            row,
            Style::default().fg(t.fade(t.text, alpha)).bg(fill),
        ));
        if pad > 0 {
            spans.push(Span::styled(" ".repeat(pad), Style::default().bg(fill)));
        }
        out.push(Line::from(spans));
    }
    out
}

fn assistant_lines(
    text: &str,
    streaming: bool,
    _finished: u64,
    app: &App,
    w: usize,
    alpha: f32,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let mut md = markdown::render(text, t, w);
    let a = if streaming { alpha.max(0.6) } else { alpha };

    if a < 1.0 {
        for line in md.iter_mut() {
            let spans = std::mem::take(&mut line.spans);
            line.spans = spans
                .into_iter()
                .map(|s| Span::styled(s.content, fade_style(s.style, t, a)))
                .collect();
        }
    }

    if streaming {
        // Comet tail: the last few glyphs lean toward the caret's color.
        let pulse = anim::breathe(app.now, 900);
        let caret_color = theme::lerp(t.accent2, t.accent, pulse);
        if let Some(last) = md.last_mut() {
            apply_comet(last, 6, caret_color, 0.7);
        }
        let caret = Span::styled("▊".to_string(), Style::default().fg(t.c(caret_color)));
        match md.last_mut() {
            Some(last) => last.spans.push(caret),
            None => md.push(Line::from(caret)),
        }
    }
    md
}

/// Rebuild a line with its last `tail` glyphs leaning toward `hot`, so newly
/// arrived text reads as bright and cools as it is pushed left.
fn apply_comet(line: &mut Line<'static>, tail: usize, hot: theme::Rgb, gain: f32) {
    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| {
            let style = s.style;
            s.content.chars().map(move |c| (c, style))
        })
        .collect();
    let n = chars.len();
    if n == 0 {
        return;
    }
    let start = n.saturating_sub(tail);
    let weights = anim::comet_weights(n - start, gain);
    let mut spans: Vec<Span> = Vec::new();
    for (i, (ch, style)) in chars.into_iter().enumerate() {
        let style = match (i >= start, style.fg) {
            (true, Some(ratatui::style::Color::Rgb(r, g, b))) => Style {
                fg: Some(ratatui::style::Color::Rgb(
                    theme::lerp((r, g, b), hot, weights[i - start]).0,
                    theme::lerp((r, g, b), hot, weights[i - start]).1,
                    theme::lerp((r, g, b), hot, weights[i - start]).2,
                )),
                ..style
            },
            _ => style,
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    line.spans = spans;
}

fn thinking_lines(app: &App, born: u64, w: usize, alpha: f32) -> Vec<Line<'static>> {
    let t = &app.theme;
    let ink = theme::thinking_color(app.status.thinking);
    let secs = (app.now.saturating_sub(born)) as f32 / 1000.0;
    let mut spans = vec![
        Span::styled(
            format!("{} ", anim::spinner(app.now)),
            Style::default().fg(t.fade(t.amber, alpha)),
        ),
        Span::styled(
            "Thinking...".to_string(),
            Style::default()
                .fg(t.fade(ink, alpha * 0.9))
                .add_modifier(Modifier::ITALIC),
        ),
    ];
    if secs >= 1.0 {
        spans.push(Span::styled(
            format!(" ({secs:.1}s)"),
            Style::default().fg(t.fade(t.faint, alpha * 0.8)),
        ));
    }
    if !app.status.thinking.is_off() && w > 36 {
        spans.push(Span::styled(
            format!(" · {}", app.status.thinking.as_str()),
            Style::default().fg(t.fade(ink, alpha * 0.7)),
        ));
    }
    vec![Line::from(spans)]
}

/// What a running tool is doing, in the present tense (opencode's idea).
fn tool_gerund(name: &str) -> &'static str {
    match name {
        "shell" => "running",
        "shell_background" => "starting",
        "bg_list" => "checking jobs",
        "bg_kill" => "stopping",
        "read_file" => "reading",
        "write_file" => "writing",
        "edit_file" | "multi_edit" => "editing",
        "create_dir" => "creating",
        "delete_file" => "deleting",
        "move_file" => "moving",
        "list_dir" => "listing",
        "file_info" => "checking",
        "search_files" => "finding files",
        "grep" => "searching",
        "web_fetch" => "fetching",
        "web_search" => "searching the web",
        "http_request" => "requesting",
        "memory_read" | "memory_write" | "memory_search" => "remembering",
        "todo_write" => "updating the list",
        "skills_list" | "skill_view" => "checking skills",
        "doctor" => "checking the tools",
        "env_status" => "checking the environment",
        _ if name.starts_with("git_") => "running git",
        _ => "working",
    }
}

/// What a tool call is about, in one line. The raw JSON is for the model,
/// not for a phone screen: show the path, the command, the pattern.
fn tool_arg(name: &str, args: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(args) else {
        return args.replace('\n', " ");
    };
    let pick = |key: &str| v.get(key).and_then(|s| s.as_str()).map(|s| s.to_string());
    let second = |a: &str, b: &str| pick(a).or_else(|| pick(b));
    let out = match name {
        "shell" | "shell_background" => second("command", "cwd"),
        "grep" | "search_files" => match (pick("pattern"), second("path", "glob")) {
            (Some(pattern), Some(where_)) => Some(format!("{pattern}  ({where_})")),
            (Some(pattern), None) => Some(pattern),
            _ => None,
        },
        "web_fetch" | "http_request" => second("url", "method"),
        "bg_kill" => v.get("id").map(|i| i.to_string()),
        "todo_write" => v
            .get("todos")
            .and_then(|t| t.as_array())
            .map(|a| format!("{} items", a.len())),
        "multi_edit" => v
            .get("edits")
            .and_then(|e| e.as_array())
            .and_then(|a| pick("path").map(|p| format!("{p}  ({} edits)", a.len()))),
        _ => second("path", "topic")
            .or_else(|| second("query", "from"))
            .or_else(|| pick("content"))
            // An unknown tool still shows something useful: its first string
            // argument.
            .or_else(|| {
                v.as_object().and_then(|o| {
                    o.values().find_map(|value| value.as_str().map(|s| s.to_string()))
                })
            }),
    };
    out.map(|s| s.replace('\n', " ")).unwrap_or_else(|| args.replace('\n', " "))
}
/// A diff line from an edit report: `+12 text`, `-4 text`, ` 7 text`.
fn diff_kind(line: &str) -> Option<char> {
    let mut chars = line.chars().peekable();
    let sign = chars.next()?;
    if !matches!(sign, '+' | '-' | ' ') {
        return None;
    }
    let mut digits = 0;
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        chars.next();
        digits += 1;
    }
    if digits == 0 || chars.next() != Some(' ') {
        return None;
    }
    Some(sign)
}

fn tool_lines(
    app: &App,
    name: &str,
    args: &str,
    state: &ToolState,
    expanded: bool,
    expand_at: u64,
    w: usize,
    alpha: f32,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let now = app.now;

    let is_running = matches!(state, ToolState::Running { .. });
    let (icon_glyph, icon_color, status, status_color) = match state {
        ToolState::Running { started } => {
            let secs = now.saturating_sub(*started) as f32 / 1000.0;
            let s = if secs > 1.5 && w > 30 {
                format!("{secs:.0}s")
            } else {
                String::new()
            };
            (anim::spinner(now), t.amber, s, t.dim)
        }
        ToolState::Done { ok, ms, at, .. } => {
            let mark = if *ok { "✓" } else { "✗" };
            let flash = 1.0 - (now.saturating_sub(*at) as f32 / 500.0).clamp(0.0, 1.0);
            let base = if *ok { t.green } else { t.red };
            (
                mark,
                base,
                format!("{mark} {}", fmt_ms(*ms)),
                theme::lerp(base, (255, 255, 255), flash * 0.8),
            )
        }
    };

    let output: Option<&str> = match state {
        ToolState::Done { output, .. } => Some(output.as_str()),
        ToolState::Running { .. } => None,
    };
    let has_body = output.is_some_and(|o| !o.trim().is_empty());
    let toggle_glyph = if expanded { "▾ " } else { "▸ " };

    let status_w = width(&status);
    let left_budget = w.saturating_sub(status_w + 3);
    let arg_line = tool_arg(name, args);

    let mut spans: Vec<Span> = Vec::new();
    if is_running {
        spans.push(Span::styled(
            format!("{icon_glyph} "),
            Style::default().fg(t.fade(icon_color, alpha)),
        ));
    } else if has_body {
        spans.push(Span::styled(
            toggle_glyph.to_string(),
            Style::default().fg(t.fade(t.faint, alpha)),
        ));
    } else {
        spans.push(Span::styled(
            format!("{icon_glyph} "),
            Style::default().fg(t.fade(icon_color, alpha)),
        ));
    }

    let label = if is_running {
        format!("{} {name}", tool_gerund(name))
    } else {
        name.to_string()
    };
    spans.push(Span::styled(
        label,
        Style::default()
            .fg(t.fade(t.text, alpha))
            .add_modifier(Modifier::BOLD),
    ));

    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    if left_budget > used + 4 {
        let arg_budget = left_budget.saturating_sub(used + 3);
        spans.push(Span::styled(
            " · ".to_string(),
            Style::default().fg(t.fade(t.faint, alpha)),
        ));
        spans.push(Span::styled(
            truncate(&arg_line, arg_budget),
            Style::default().fg(t.fade(t.dim, alpha)),
        ));
    }

    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    let pad = w.saturating_sub(used + status_w + 1);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    spans.push(Span::styled(
        status,
        Style::default().fg(t.fade(status_color, alpha)),
    ));
    let mut out = vec![Line::from(spans)];

    let inner_w = w.saturating_sub(CARD_INDENT).max(4);
    match (expanded, output) {
        (false, Some(text)) if has_body => {
            let mut shown = text
                .trim()
                .lines()
                .map(|l| l.trim())
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .to_string();
            let more = text.trim().lines().filter(|l| !l.trim().is_empty()).count() > 1;
            let budget = inner_w;
            shown = truncate(&shown, budget);
            if more && width(&shown) < budget {
                shown.push_str(" …");
            }
            out.push(Line::from(vec![
                Span::raw(" ".repeat(CARD_INDENT)),
                Span::styled(
                    shown,
                    Style::default().fg(t.fade(t.faint, alpha)),
                ),
            ]));
        }
        (true, Some(text)) => {
            let reveal_tween = Tween::new(240, anim::Ease::OutCubic);
            let reveal = reveal_tween.t(now, expand_at);
            let body: Vec<&str> = text.split('\n').collect();
            let total = body.len();
            let shown = anim::reveal_lines(body.len().min(MAX_CARD_LINES), reveal);
            for line in body.iter().take(shown) {
                let ink = match diff_kind(line) {
                    Some('+') => t.green,
                    Some('-') => t.red,
                    Some(_) => t.faint,
                    None if line.trim() == "…" => t.faint,
                    None => t.dim,
                };
                for row in wrap(line, inner_w).into_iter() {
                    out.push(Line::from(vec![
                        Span::raw(" ".repeat(CARD_INDENT)),
                        Span::styled(
                            row,
                            Style::default().fg(t.fade(ink, alpha)),
                        ),
                    ]));
                }
            }
            if reveal_tween.done(now, expand_at) && total > MAX_CARD_LINES {
                out.push(Line::from(vec![
                    Span::raw(" ".repeat(CARD_INDENT)),
                    Span::styled(
                        format!("… {} more lines", total - MAX_CARD_LINES),
                        Style::default().fg(t.fade(t.faint, alpha)),
                    ),
                ]));
            }
        }
        _ => {}
    }
    out
}

fn note_lines(
    app: &App,
    text: &str,
    error: bool,
    born: u64,
    alpha: f32,
    w: usize,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let base = if error { t.red } else { t.faint };
    // An error flashes bright for a moment before it settles into red.
    let flash = if error {
        1.0 - (app.now.saturating_sub(born) as f32 / 600.0).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let color = theme::lerp(base, (255, 255, 255), flash * 0.45);
    let mut spans = vec![Span::styled(
        "  · ".to_string(),
        Style::default().fg(t.fade(color, alpha)),
    )];
    for (i, row) in wrap(text, w.saturating_sub(4)).into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("    ".to_string()));
        }
        spans.push(Span::styled(row, Style::default().fg(t.fade(color, alpha))));
    }
    vec![Line::from(spans)]
}

// -- helpers ---------------------------------------------------------------

/// Rebuild a line fading every color toward `toward` by `1 - keep`.
fn fade_line(line: Line<'static>, toward: theme::Rgb, keep: f32) -> Line<'static> {
    let spans: Vec<Span> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content, fade_to(s.style, toward, keep)))
        .collect();
    Line::from(spans)
}

fn fade_to(style: Style, toward: theme::Rgb, keep: f32) -> Style {
    let fg = match style.fg {
        Some(ratatui::style::Color::Rgb(r, g, b)) => {
            Some(ratatui::style::Color::Rgb(
                theme::lerp((r, g, b), toward, 1.0 - keep).0,
                theme::lerp((r, g, b), toward, 1.0 - keep).1,
                theme::lerp((r, g, b), toward, 1.0 - keep).2,
            ))
        }
        other => other,
    };
    let bg = match style.bg {
        Some(ratatui::style::Color::Rgb(r, g, b)) => Some(ratatui::style::Color::Rgb(
            theme::lerp((r, g, b), toward, (1.0 - keep) * 0.5).0,
            theme::lerp((r, g, b), toward, (1.0 - keep) * 0.5).1,
            theme::lerp((r, g, b), toward, (1.0 - keep) * 0.5).2,
        )),
        other => other,
    };
    Style {
        fg,
        bg,
        add_modifier: style.add_modifier,
        sub_modifier: style.sub_modifier,
        underline_color: style.underline_color,
    }
}

fn fade_style(style: Style, t: &Theme, alpha: f32) -> Style {
    let fg = match style.fg {
        Some(ratatui::style::Color::Rgb(r, g, b)) => Some(t.fade((r, g, b), alpha)),
        other => other,
    };
    let bg = match style.bg {
        Some(ratatui::style::Color::Rgb(r, g, b)) => Some(t.fade((r, g, b), alpha.max(0.6))),
        other => other,
    };
    Style {
        fg,
        bg,
        add_modifier: style.add_modifier,
        sub_modifier: style.sub_modifier,
        underline_color: style.underline_color,
    }
}

fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f32 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::theme::ColorLevel;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app_with(items: Vec<Item>) -> App {
        let mut app = App::new(Theme::forced(ColorLevel::True));
        app.phase = crate::app::Phase::Chat;
        app.now = 5_000;
        app.chat_at = 0;
        app.items = items;
        app
    }

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, f.area(), app)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_messages_on_narrow_screen() {
        let app = app_with(vec![
            Item::User { text: "hello there".into(), born: 0 },
            Item::Assistant {
                text: "hi **you**".into(),
                born: 0,
                streaming: false,
                finished: 0,
            },
        ]);
        let out = render(&app, 40, 12);
        assert!(out.contains("hello there"));
        assert!(out.contains("hi you"));
    }

    #[test]
    fn tool_card_shows_result() {
        let app = app_with(vec![Item::Tool {
            name: "bash".into(),
            args: "git status".into(),
            state: ToolState::Done {
                ok: true,
                ms: 42,
                output: "clean".into(),
                at: 4_000,
            },
            born: 0,
            expanded: false,
            expand_at: 0,
        }]);
        let out = render(&app, 44, 8);
        assert!(out.contains("bash"));
        assert!(out.contains("git status"));
        assert!(out.contains("42ms"));
        assert!(out.contains("clean"), "collapsed card should preview output");
    }

    #[test]
    fn failed_card_shows_cross() {
        let app = app_with(vec![Item::Tool {
            name: "shell".into(),
            args: "false".into(),
            state: ToolState::Done {
                ok: false,
                ms: 5,
                output: "exit 1".into(),
                at: 4_900,
            },
            born: 0,
            expanded: false,
            expand_at: 0,
        }]);
        let out = render(&app, 44, 8);
        assert!(out.contains("✗"));
    }

    #[test]
    fn streaming_shows_caret() {
        let app = app_with(vec![Item::Assistant {
            text: "partial answer".into(),
            born: 0,
            streaming: true,
            finished: 0,
        }]);
        let out = render(&app, 40, 6);
        assert!(out.contains("▊"));
    }

    #[test]
    fn thinking_draws_indicator() {
        let app = app_with(vec![Item::Thinking { born: 4_000 }]);
        let out = render(&app, 44, 8);
        assert!(out.contains("Thinking"), "{out}");
        assert!(!out.contains("▁"), "clean single-line indicator without skeleton clutter");
    }

    #[test]
    fn tool_args_show_what_they_are_about() {
        assert_eq!(tool_arg("shell", r#"{"command":"git status"}"#), "git status");
        assert_eq!(tool_arg("read_file", r#"{"path":"src/main.rs"}"#), "src/main.rs");
        assert_eq!(
            tool_arg("grep", r#"{"pattern":"fn main","path":"src"}"#),
            "fn main  (src)"
        );
        assert_eq!(tool_arg("web_fetch", r#"{"url":"https://example.com"}"#), "https://example.com");
        // Anything unrecognised falls back to the raw arguments rather than
        // showing nothing at all.
        assert_eq!(tool_arg("mystery", r#"{"odd":"value"}"#), "value");
        assert_eq!(tool_arg("mystery", "not json"), "not json");
    }

    #[test]
    fn diff_lines_are_recognized() {
        assert_eq!(diff_kind("+12 hello"), Some('+'));
        assert_eq!(diff_kind("-4 gone"), Some('-'));
        assert_eq!(diff_kind(" 7 same"), Some(' '));
        assert_eq!(diff_kind("plain text"), None);
        assert_eq!(diff_kind("+nope"), None);
        assert_eq!(diff_kind("…"), None);
    }

    #[test]
    fn a_finished_answer_keeps_one_thought_line() {
        let mut app = app_with(vec![
            Item::Reasoning { text: "weighing options".into(), born: 0, finished: 2_500 },
            Item::Assistant {
                text: "here is the answer".into(),
                born: 2_500,
                streaming: false,
                finished: 3_000,
            },
        ]);
        let out = render(&app, 44, 10);
        assert!(out.contains("Thought"), "{out}");
        assert!(out.contains("2s"), "the thought line says how long it took: {out}");
        assert!(!out.contains("weighing options"), "collapsed by default: {out}");

        // ctrl+t opens it again.
        app.thinking_open = true;
        let out = render(&app, 44, 10);
        assert!(out.contains("weighing options"), "{out}");
    }

    #[test]
    fn empty_state_shows_welcome() {
        let app = app_with(vec![]);
        let out = render(&app, 44, 16);
        assert!(out.contains("harness"));
        assert!(out.contains("/doctor"));
    }

    #[test]
    fn expanded_card_shows_output_lines() {
        let app = app_with(vec![Item::Tool {
            name: "bash".into(),
            args: "ls".into(),
            state: ToolState::Done {
                ok: true,
                ms: 3,
                output: "alpha\nbeta".into(),
                at: 0,
            },
            born: 0,
            expanded: true,
            expand_at: 0,
        }]);
        let out = render(&app, 44, 8);
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
        assert!(out.contains("▾"));
    }

    #[test]
    fn visual_layout_inspection() {
        let app = app_with(vec![
            Item::User { text: "how do I list files?".into(), born: 0 },
            Item::Reasoning {
                text: "The user is asking how to list files in Linux or Android. I should suggest ls or find.".into(),
                born: 1_000,
                finished: 3_200,
            },
            Item::Tool {
                name: "shell".into(),
                args: r#"{"command":"ls -la"}"#.into(),
                state: ToolState::Done {
                    ok: true,
                    ms: 18,
                    output: "total 4\n-rw-r--r-- 1 user 100 file.txt".into(),
                    at: 3_500,
                },
                born: 3_200,
                expanded: false,
                expand_at: 0,
            },
            Item::Assistant {
                text: "# File Listing\n\nYou can use `ls` to list directory contents:\n\n```bash\nls -la\n```\n\nKey flags:\n- `-l`: long listing format\n- `-a`: include hidden files".into(),
                born: 3_500,
                streaming: false,
                finished: 4_500,
            },
        ]);
        let out = render(&app, 44, 24);
        let out_narrow = render(&app, 36, 24);
        let mut app_open = app;
        app_open.thinking_open = true;
        let out_open = render(&app_open, 44, 24);
        println!("=== VISUAL RENDER (44 cols) ===\n{out}\n===============================");
        println!("=== VISUAL RENDER (36 cols) ===\n{out_narrow}\n===============================");
        println!("=== VISUAL RENDER (OPEN THINKING) ===\n{out_open}\n===============================");
        assert!(out.contains("how do I list files?"));
        assert!(out.contains("Thought for 2s"));
        assert!(!out.contains("│ Thought"), "no vertical pipe on collapsed thought");
        assert!(out.contains("File Listing"));
        assert!(out.contains("```bash"));
        assert!(out.contains("Key flags:"));
        assert!(out.contains("• -l:"));
        assert!(out_narrow.contains("Thought for 2s"));
        assert!(out_open.contains("▾ Thought for 2s"));
        assert!(out_open.contains("The user is asking how to list files"));
    }
}
