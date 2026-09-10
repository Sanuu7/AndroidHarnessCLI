//! Fixed furniture: logo row, the rule above the composer, the composer, and
//! the status bar.

use crate::anim::{self, breathe, saw};
use crate::app::App;
use crate::input::View as InputView;
use crate::textutil::{short_num, truncate, width};
use crate::theme;
use crate::ui::{MID, WIDE};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub fn header(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled("◆ ", Style::default().fg(t.c(t.accent))));
    for (i, ch) in "harness".chars().enumerate() {
        let p = i as f32 / 6.0;
        spans.push(Span::styled(
            ch.to_string(),
            Style::default()
                .fg(t.c(theme::grad(t.accent, t.accent2, p)))
                .add_modifier(Modifier::BOLD),
        ));
    }

    let left_w: usize = 10;
    let right = format!("{} ", app.status.workspace);
    let right = truncate(&right, (area.width as usize).saturating_sub(left_w + 2));
    let right_w = width(&right);
    let pad = (area.width as usize)
        .saturating_sub(left_w)
        .saturating_sub(right_w);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(right, Style::default().fg(t.c(t.faint))));

    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

pub fn separator(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let w = area.width as usize;
    let cols = w.min(area.width as usize);
    let spans: Vec<Span> = if app.busy {
        let offset = saw(app.now, 2200);
        let colors = anim::sweep_colors(&[t.accent2, t.accent, t.faint], cols, offset);
        colors
            .into_iter()
            .map(|c| Span::styled("─".to_string(), Style::default().fg(t.c(c))))
            .collect()
    } else {
        (0..cols)
            .map(|i| {
                let p = i as f32 / (cols.max(2) - 1) as f32;
                let c = theme::grad(t.faint, t.bg, p.powf(1.6));
                Span::styled("─".to_string(), Style::default().fg(t.c(c)))
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn input(frame: &mut Frame, area: Rect, app: &App, iv: &InputView) {
    if app.input.is_empty() {
        let t = &app.theme;
        // A quiet hint where the text will be typed.
        let placeholder = if area.width >= MID {
            "ask anything  ·  / for commands"
        } else {
            "ask anything"
        };
        let line = Line::from(vec![
            Span::styled("› ".to_string(), Style::default().fg(t.c(t.accent))),
            Span::styled(
                truncate(placeholder, area.width.saturating_sub(3) as usize),
                Style::default().fg(t.c(t.faint)),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    } else {
        frame.render_widget(Paragraph::new(ratatui::text::Text::from(iv.rows.clone())), area);
    }
}

fn fmt_cost(cost: f64, compact: bool) -> String {
    if cost <= 0.0 {
        return "$0".to_string();
    }
    if cost < 1.0 {
        let s = format!("{cost:.4}");
        if compact {
            // Drop the leading zero, terminals are narrow.
            return format!("${}", s.trim_start_matches("0"));
        }
        return format!("${s}");
    }
    format!("${cost:.2}")
}

pub fn status(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let w = area.width as usize;
    let s = &app.status;

    let mut left: Vec<Span> = Vec::new();
    if app.busy {
        left.push(Span::styled(
            format!("{} ", anim::spinner(app.now)),
            Style::default().fg(t.c(t.accent)),
        ));
    } else {
        let dot = theme::lerp(t.faint, t.accent, breathe(app.now, 3000) * 0.35);
        left.push(Span::styled("· ", Style::default().fg(t.c(dot))));
    }
    left.push(Span::styled(
        s.model.clone(),
        Style::default().fg(t.c(t.dim)),
    ));

    // Cost flashes toward white when it changes.
    let flash = 1.0 - (app.now.saturating_sub(s.cost_flash) as f32 / 700.0).clamp(0.0, 1.0);
    let cost_color = theme::lerp(t.green, (255, 255, 255), flash * 0.7);

    let mut right: Vec<Span> = Vec::new();
    if w >= MID as usize {
        right.push(Span::styled(
            format!(
                "↑{} ↓{}  ",
                short_num(s.tokens_in),
                short_num(s.tokens_out)
            ),
            Style::default().fg(t.c(t.faint)),
        ));
    }
    let pct = if s.ctx_max == 0 {
        0
    } else {
        ((s.tokens_in + s.tokens_out) * 100 / s.ctx_max) as u32
    };
    if w >= 30 {
        let ctx_color = if pct > 80 { t.amber } else { t.faint };
        right.push(Span::styled(
            format!("{pct}%  "),
            Style::default().fg(t.c(ctx_color)),
        ));
    }
    right.push(Span::styled(
        format!("{}  ", fmt_cost(s.cost, w < WIDE as usize)),
        Style::default().fg(t.c(cost_color)),
    ));

    let left_w: usize = left.iter().map(|s| width(&s.content)).sum();
    let right_w: usize = right.iter().map(|s| width(&s.content)).sum();
    let pad = w.saturating_sub(left_w + right_w);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
