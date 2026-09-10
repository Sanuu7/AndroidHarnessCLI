//! The transcript: messages, thinking, tool cards.
//!
//! Lines are rebuilt each frame from the item list. That is cheap at chat
//! scale and keeps every animation (fade in, shimmer, card reveal, caret
//! pulse) a pure function of `app.now`.

use crate::anim::{self, Tween};
use crate::app::{App, Item, ToolState};
use crate::markdown;
use crate::textutil::{pad_right, truncate, width, wrap};
use crate::theme::{self, Theme};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let w = area.width as usize;
    let scale = app.appear(app.chat_at).max(0.15);
    let mut lines: Vec<Line> = Vec::new();
    for item in &app.items {
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
    let visible: Vec<Line> = lines.into_iter().skip(start).take(h).collect();

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
        Item::Note { text, error, .. } => {
            let color = if *error { t.red } else { t.faint };
            let mut spans = vec![Span::styled(
                "  · ".to_string(),
                Style::default().fg(t.fade(color, alpha)),
            )];
            for (i, row) in wrap(text, w.saturating_sub(4)).into_iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw("    ".to_string()));
                }
                spans.push(Span::styled(
                    row,
                    Style::default().fg(t.fade(color, alpha)),
                ));
            }
            vec![Line::from(spans)]
        }
    }
}

/// `▌you first line` with the rest hanging under the text column.
const USER_INDENT: usize = 5;

fn user_lines(text: &str, t: &Theme, w: usize, alpha: f32) -> Vec<Line<'static>> {
    let body_w = w.saturating_sub(USER_INDENT).max(8);
    let head = vec![
        Span::styled(
            "▌".to_string(),
            Style::default().fg(t.fade(t.accent, alpha)),
        ),
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
    let body_w = w.saturating_sub(2);
    let mut md = markdown::render(text, t, body_w);
    // The whole message fades in while streaming, and sits at full strength
    // once it is done.
    let a = if streaming { alpha.max(0.55) } else { alpha };

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
            spans.push(Span::raw("  ".to_string()));
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
        // Caret pulses, and settles into place when the stream ends.
        let pulse = anim::breathe(app.now, 900);
        let caret_color = theme::lerp(t.accent2, t.accent, pulse);
        let caret = Span::styled(
            "▊".to_string(),
            Style::default().fg(t.c(caret_color)),
        );
        match md.last_mut() {
            Some(last) => last.spans.push(caret),
            None => md.push(Line::from(caret)),
        }
    } else if finished > 0 {
        // Little settle flash right after the stream stops: a beat, then the
        // caret's replacement dot cools back to dim.
        let settle = Tween::delayed(140, 520, anim::Ease::OutQuint);
        if !settle.done(app.now, finished) {
            let c = theme::lerp(t.accent2, t.dim, settle.t(app.now, finished));
            let dot = Span::styled("·".to_string(), Style::default().fg(t.c(c)));
            match md.last_mut() {
                Some(last) => last.spans.push(dot),
                None => md.push(Line::from(dot)),
            }
        }
    }
    md
}

fn thinking_lines(app: &App, born: u64, w: usize, alpha: f32) -> Vec<Line<'static>> {
    let t = &app.theme;
    let label = "thinking";
    let cols = label.chars().count();
    let phase = anim::saw(app.now, 1600);
    let colors = anim::shimmer_colors(t.dim, t.accent, cols, phase, 2.2);
    let mut spans = vec![
        Span::styled(
            "  ".to_string(),
            Style::default().fg(t.fade(t.faint, alpha)),
        ),
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
    vec![Line::from(spans)]
}

#[allow(clippy::too_many_arguments)]
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
    let right = match state {
        ToolState::Running { started } => {
            let secs = app.now.saturating_sub(*started) as f32 / 1000.0;
            if secs > 1.5 && w > 30 {
                format!("{} {secs:.0}s", anim::spinner(app.now))
            } else {
                anim::spinner(app.now).to_string()
            }
        }
        ToolState::Done { ok, ms, .. } => {
            let mark = if *ok { "✓" } else { "✗" };
            format!("{mark} {}", fmt_ms(*ms))
        }
    };
    let right_color = match state {
        ToolState::Running { .. } => t.dim,
        ToolState::Done { ok: true, .. } => t.green,
        ToolState::Done { ok: false, .. } => t.red,
    };
    let right_w = width(&right);
    let left_budget = w.saturating_sub(right_w + 2);

    let mut left: Vec<Span> = vec![
        Span::styled(
            "▣ ".to_string(),
            Style::default().fg(t.fade(t.accent2, alpha)),
        ),
        Span::styled(
            name.to_string(),
            Style::default()
                .fg(t.fade(t.text, alpha))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let name_w = width(name) + 2;
    if left_budget > name_w + 3 {
        let arg_budget = left_budget.saturating_sub(name_w + 3);
        let arg = truncate(args, arg_budget);
        left.push(Span::styled(
            " · ".to_string(),
            Style::default().fg(t.fade(t.faint, alpha)),
        ));
        left.push(Span::styled(
            arg,
            Style::default().fg(t.fade(t.dim, alpha)),
        ));
    }
    let used: usize = left.iter().map(|s| width(&s.content)).sum();
    let pad = w.saturating_sub(used + right_w + 1);
    left.push(Span::raw(" ".repeat(pad)));
    left.push(Span::styled(
        right,
        Style::default().fg(t.fade(right_color, alpha)),
    ));
    let mut out = vec![Line::from(left)];

    if expanded {
        let reveal_tween = Tween::new(240, anim::Ease::OutCubic);
        let reveal = reveal_tween.t(app.now, expand_at);
        let mut body: Vec<String> = Vec::new();
        match state {
            ToolState::Running { .. } => {
                body.push(format!("$ {args}"));
                body.push("running…".to_string());
            }
            ToolState::Done { output, .. } => {
                body.push(format!("$ {args}"));
                for l in output.split('\n') {
                    body.push(l.to_string());
                }
            }
        }
        let total = body.len();
        let cap = 8usize;
        let shown_lines = body.len().min(cap);
        let n = anim::reveal_lines(shown_lines, reveal);
        let inner_w = w.saturating_sub(6);
        for line in body.iter().take(n) {
            for (i, row) in wrap(line, inner_w).into_iter().enumerate() {
                let lead = if i == 0 { "    " } else { "      " };
                out.push(Line::from(vec![
                    Span::raw(lead.to_string()),
                    Span::styled(
                        pad_right(&row, inner_w),
                        Style::default()
                            .fg(t.fade(t.dim, alpha))
                            .bg(t.c(t.card_bg)),
                    ),
                ]));
            }
        }
        if reveal_tween.done(app.now, expand_at) && total > cap {
            out.push(Line::from(Span::styled(
                format!("    … {} more lines", total - cap),
                Style::default().fg(t.fade(t.faint, alpha)),
            )));
        }
    }
    out
}

fn fade_style(style: Style, t: &Theme, alpha: f32) -> Style {
    let fg = match style.fg {
        Some(ratatui::style::Color::Rgb(r, g, b)) => Some(t.fade((r, g, b), alpha)),
        other => other,
    };
    let bg = match style.bg {
        Some(ratatui::style::Color::Rgb(r, g, b)) => {
            Some(t.fade(theme::lerp((r, g, b), (0, 0, 0), 0.0), alpha.max(0.6)))
        }
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
            },
            born: 0,
            expanded: false,
            expand_at: 0,
        }]);
        let out = render(&app, 44, 8);
        assert!(out.contains("bash"));
        assert!(out.contains("git status"));
        assert!(out.contains("42ms"));
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
}
