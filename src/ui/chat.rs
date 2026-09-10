//! The transcript: messages, thinking, tool cards, and the welcome screen.
//!
//! Lines are rebuilt each frame from the item list. That is cheap at chat
//! scale and keeps every animation (fade in, comet tail, skeleton shimmer,
//! card settle, completion sweep) a pure function of `app.now`.

use crate::anim::{self, Tween};
use crate::app::{App, Item, ToolState};
use crate::markdown;
use crate::textutil::{pad_right, truncate, width, wrap};
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

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    if app.is_empty_state() {
        welcome(frame, area, app);
        return;
    }
    let w = area.width as usize;
    let scale = app.appear(app.chat_at).max(0.15);
    let mut lines: Vec<Line> = Vec::new();
    for item in app.items.iter() {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(item_lines(item, app, w, scale));
    }

    let total = lines.len();
    let h = area.height as usize;
    let max_scroll = total.saturating_sub(h);
    let scroll = app.scroll.min(max_scroll);
    let start = total.saturating_sub(h + scroll);
    let mut visible: Vec<Line> = lines.into_iter().skip(start).take(h).collect();

    // When there is more above, the top rows fade out instead of being cut.
    if start > 0 {
        let mask_rows = 2.min(visible.len());
        for (row, line) in visible.iter_mut().take(mask_rows).enumerate() {
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
        draw_scroll_hint(frame, area, app, scroll);
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

fn user_lines(text: &str, t: &Theme, w: usize, alpha: f32) -> Vec<Line<'static>> {
    let body_w = w.saturating_sub(USER_INDENT).max(8);
    let head = vec![
        Span::styled("▌".to_string(), Style::default().fg(t.fade(t.accent, alpha))),
        Span::styled(
            "you".to_string(),
            Style::default()
                .fg(t.fade(t.dim, alpha))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" ".to_string()),
    ];
    let mut out = Vec::new();
    for (i, row) in wrap(text, body_w).into_iter().enumerate() {
        let mut spans = if i == 0 {
            head.clone()
        } else {
            vec![Span::raw(" ".repeat(USER_INDENT))]
        };
        spans.push(Span::styled(
            row,
            Style::default().fg(t.fade(t.text, alpha)),
        ));
        out.push(Line::from(spans));
    }
    out
}

fn assistant_lines(
    text: &str,
    streaming: bool,
    finished: u64,
    app: &App,
    w: usize,
    alpha: f32,
) -> Vec<Line<'static>> {
    let t = &app.theme;
    let body_w = w.saturating_sub(BODY_INDENT);
    let mut md = markdown::render(text, t, body_w);
    // A streaming message stays readable while it grows in; the rest fades in
    // normally and sits at full strength once it is done.
    let a = if streaming { alpha.max(0.6) } else { alpha };

    let bar = Span::styled(
        "▌".to_string(),
        Style::default().fg(t.fade(t.accent2, a)),
    );
    for (i, line) in md.iter_mut().enumerate() {
        let mut spans = Vec::new();
        if i == 0 {
            spans.push(bar.clone());
            spans.push(Span::raw(" ".to_string()));
        } else {
            spans.push(Span::raw(" ".repeat(BODY_INDENT)));
        }
        spans.extend(line.spans.drain(..));
        if a < 1.0 {
            spans = spans
                .into_iter()
                .map(|s| Span::styled(s.content, fade_style(s.style, t, a)))
                .collect();
        }
        *line = Line::from(spans);
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
    } else if finished > 0 {
        // The caret's place is taken by a dot that cools off, and a rule
        // sweeps out under the message to say it landed.
        let settle = Tween::delayed(140, 520, anim::Ease::OutQuint);
        let sweep = Tween::new(560, anim::Ease::OutQuint);
        if !settle.done(app.now, finished) {
            let c = theme::lerp(t.accent2, t.dim, settle.t(app.now, finished));
            let dot = Span::styled("·".to_string(), Style::default().fg(t.c(c)));
            match md.last_mut() {
                Some(last) => last.spans.push(dot),
                None => md.push(Line::from(dot)),
            }
        } else if !sweep.done(app.now, finished) {
            let t_sweep = sweep.t(app.now, finished);
            let cols = (body_w as f32 * t_sweep).round() as usize;
            let alpha_out = 1.0 - ((t_sweep - 0.6) / 0.4).clamp(0.0, 1.0);
            let spans: Vec<Span> = (0..cols)
                .map(|i| {
                    let f = i as f32 / cols.max(1) as f32;
                    let c = theme::grad(t.accent2, t.accent, f);
                    Span::styled("─".to_string(), Style::default().fg(t.fade(c, alpha_out)))
                })
                .collect();
            md.push(Line::from(spans));
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
    let label = "thinking";
    let cols = label.chars().count();
    let phase = anim::saw(app.now, 1600);
    let colors = anim::shimmer_colors(t.dim, t.accent, cols, phase, 2.2);
    let mut spans = vec![
        Span::raw("  ".to_string()),
        Span::styled(
            format!("{} ", anim::spinner(app.now)),
            Style::default().fg(t.fade(t.amber, alpha)),
        ),
    ];
    for (ch, color) in label.chars().zip(colors) {
        spans.push(Span::styled(
            ch.to_string(),
            Style::default().fg(t.fade(color, alpha)),
        ));
    }
    let secs = (app.now.saturating_sub(born)) as f32 / 1000.0;
    if secs > 2.0 && w > 30 {
        spans.push(Span::styled(
            format!("  {secs:.0}s"),
            Style::default().fg(t.fade(t.faint, alpha)),
        ));
    }
    let mut out = vec![Line::from(spans)];

    // Two skeleton rows, shimmering, so the wait has something alive in it.
    let skel_phase = anim::saw(app.now, 1900);
    for (row, len_frac) in [(0usize, 1.0f32), (1usize, 0.62f32)] {
        let cols = ((w.saturating_sub(CARD_INDENT)) as f32 * len_frac).round() as usize;
        let cols = cols.min(38);
        let colors =
            anim::skeleton_colors(cols, (skel_phase + row as f32 * 0.12).fract(), 4.0, t.faint, t.accent);
        let mut s = vec![Span::raw(" ".repeat(BODY_INDENT))];
        for color in colors {
            s.push(Span::styled(
                "▁".to_string(),
                Style::default().fg(t.fade(color, alpha * 0.5)),
            ));
        }
        out.push(Line::from(s));
    }
    out
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

    let (gutter_color, status, status_color) = match state {
        ToolState::Running { started } => {
            // The spinner already sits in the icon slot, so this side shows
            // only how long it has been going.
            let secs = now.saturating_sub(*started) as f32 / 1000.0;
            let s = if secs > 1.5 && w > 30 {
                format!("{secs:.0}s")
            } else {
                String::new()
            };
            (t.amber, s, t.dim)
        }
        ToolState::Done { ok, ms, at, .. } => {
            let mark = if *ok { "✓" } else { "✗" };
            // The result flashes bright, then cools to its state color.
            let flash = 1.0 - (now.saturating_sub(*at) as f32 / 500.0).clamp(0.0, 1.0);
            let base = if *ok { t.green } else { t.red };
            (
                if *ok { t.green } else { t.red },
                format!("{mark} {}", fmt_ms(*ms)),
                theme::lerp(base, (255, 255, 255), flash * 0.8),
            )
        }
    };

    // A failed card wobbles for a moment.
    let jitter = match state {
        ToolState::Done { ok: false, at, .. } => anim::shake(now, *at, 420).unsigned_abs() as usize,
        _ => 0,
    };

    let output: Option<&str> = match state {
        ToolState::Done { output, .. } => Some(output.as_str()),
        ToolState::Running { .. } => None,
    };
    let has_body = output.is_some_and(|o| !o.trim().is_empty());
    let glyph = if expanded { "▾" } else { "▸" };

    let status_w = width(&status);
    let left_budget = w.saturating_sub(status_w + 3 + jitter);
    let mut spans: Vec<Span> = Vec::new();
    if jitter > 0 {
        spans.push(Span::raw(" ".repeat(jitter)));
    }
    spans.push(Span::styled(
        "▎".to_string(),
        Style::default().fg(t.fade(gutter_color, alpha)),
    ));
    if matches!(state, ToolState::Running { .. }) {
        spans.push(Span::styled(
            format!("{} ", anim::spinner(now)),
            Style::default().fg(t.fade(t.amber, alpha)),
        ));
    } else if has_body {
        spans.push(Span::styled(
            format!("{glyph} "),
            Style::default().fg(t.fade(t.faint, alpha)),
        ));
    } else {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        name.to_string(),
        Style::default()
            .fg(t.fade(t.text, alpha))
            .add_modifier(Modifier::BOLD),
    ));
    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    if left_budget > used + 5 {
        let arg_budget = left_budget.saturating_sub(used + 3);
        spans.push(Span::styled(
            " · ".to_string(),
            Style::default().fg(t.fade(t.faint, alpha)),
        ));
        spans.push(Span::styled(
            truncate(args, arg_budget),
            Style::default().fg(t.fade(t.dim, alpha)),
        ));
    }
    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    let pad = w.saturating_sub(used + status_w + 1);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(
        status,
        Style::default().fg(t.fade(status_color, alpha)),
    ));
    let mut out = vec![Line::from(spans)];

    // Collapsed cards keep one line of result visible: the useful part.
    match (expanded, output) {
        (false, Some(text)) if has_body => {
            let mut shown = text.trim().lines().next().unwrap_or("").trim().to_string();
            let more = text.trim().lines().count() > 1;
            let budget = w.saturating_sub(CARD_INDENT + 2);
            shown = truncate(&shown, budget);
            if more && width(&shown) < budget {
                shown.push(' ');
                shown.push('…');
            }
            out.push(Line::from(vec![
                Span::raw(" ".repeat(CARD_INDENT)),
                Span::styled(shown, Style::default().fg(t.fade(t.faint, alpha))),
            ]));
        }
        (true, Some(text)) => {
            let reveal_tween = Tween::new(240, anim::Ease::OutCubic);
            let reveal = reveal_tween.t(now, expand_at);
            let mut body: Vec<&str> = vec![];
            body.extend(text.split('\n'));
            let total = body.len();
            let shown = anim::reveal_lines(body.len().min(10), reveal);
            let inner_w = w.saturating_sub(CARD_INDENT + 2).max(4);
            for line in body.iter().take(shown) {
                for (i, row) in wrap(line, inner_w).into_iter().enumerate() {
                    let lead = if i == 0 { CARD_INDENT } else { CARD_INDENT + 2 };
                    out.push(Line::from(vec![
                        Span::raw(" ".repeat(lead)),
                        Span::styled(
                            pad_right(&row, inner_w),
                            Style::default()
                                .fg(t.fade(t.dim, alpha))
                                .bg(t.c(t.card_bg)),
                        ),
                    ]));
                }
            }
            if reveal_tween.done(now, expand_at) && total > 10 {
                out.push(Line::from(Span::styled(
                    format!("{}… {} more lines", " ".repeat(CARD_INDENT), total - 10),
                    Style::default().fg(t.fade(t.faint, alpha)),
                )));
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
    fn thinking_draws_skeleton() {
        let app = app_with(vec![Item::Thinking { born: 4_000 }]);
        let out = render(&app, 44, 8);
        assert!(out.contains("thinking"));
        assert!(out.contains("▁"), "skeleton rows should be drawn");
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
}
