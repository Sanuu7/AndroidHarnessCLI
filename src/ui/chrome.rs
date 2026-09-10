//! Fixed furniture: logo row, the rule above the composer, the composer, and
//! the status bar.

use crate::anim::{self, breathe, saw, Tween};
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
    let w = area.width as usize;
    // The gradient drifts slowly, so the header is never quite still.
    let drift = saw(app.now, 9000) * 0.5;
    let mut spans: Vec<Span> = Vec::new();
    let diamond = if app.busy {
        breathe(app.now, 900)
    } else {
        breathe(app.now, 3200)
    };
    spans.push(Span::styled(
        "◆ ",
        Style::default().fg(t.c(theme::lerp(t.accent, (255, 255, 255), diamond * 0.45))),
    ));
    for (i, ch) in "harness".chars().enumerate() {
        let p = (i as f32 / 6.0 + drift).fract();
        spans.push(Span::styled(
            ch.to_string(),
            Style::default()
                .fg(t.c(theme::ramp(&[t.accent, t.accent2, t.accent], p)))
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Where the work is happening, with the branch when there is one, the
    // way pi's footer shows pwd and branch together.
    let left_w = 10;
    let mut where_ = app.status.workspace.clone();
    if !app.status.branch.is_empty() {
        where_.push_str(&format!(" ({})", app.status.branch));
    }
    let right = format!("{where_} ");
    let right = truncate(&right, w.saturating_sub(left_w + 2));
    let right_w = width(&right);
    let pad = w.saturating_sub(left_w).saturating_sub(right_w);
    spans.push(Span::raw(" ".repeat(pad)));
    // The branch is the part worth reading, so it stays bright.
    let (name, branch) = match right.split_once(" (") {
        Some((name, branch)) => (name.to_string(), Some(branch.trim_end_matches(") ").trim().to_string())),
        None => (right.trim_end().to_string(), None),
    };
    spans.push(Span::styled(name, Style::default().fg(t.c(t.faint))));
    if let Some(branch) = branch {
        spans.push(Span::styled(format!(" ({branch})"), Style::default().fg(t.c(t.dim))));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn separator(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let cols = area.width as usize;
    let now = app.now;

    // Sending pulses a bright band once; cancelling flashes the rule red.
    let send_band = if app.sent_at > 0 && now.saturating_sub(app.sent_at) < 700 {
        Some(Tween::new(700, anim::Ease::OutCubic).t(now, app.sent_at))
    } else {
        None
    };
    let cancel_k = if app.cancel_at > 0 && now.saturating_sub(app.cancel_at) < 500 {
        1.0 - (now.saturating_sub(app.cancel_at) as f32 / 500.0)
    } else {
        0.0
    };

    let spans: Vec<Span> = if app.busy {
        let offset = saw(now, 2200);
        let stops: &[theme::Rgb] = &[t.accent2, t.accent, t.faint];
        anim::sweep_colors(stops, cols, offset)
            .into_iter()
            .map(|c| Span::styled("─".to_string(), Style::default().fg(t.c(c))))
            .collect()
    } else {
        (0..cols)
            .map(|i| {
                let p = i as f32 / (cols.max(2) - 1) as f32;
                let mut c = theme::grad(t.faint, t.bg, p.powf(1.6));
                if let Some(band) = send_band {
                    // One bright sweep travelling outward after a send.
                    let center = band * cols as f32;
                    let w = anim::bell(i as f32, center, 6.0);
                    c = theme::lerp(c, t.accent, w * (1.0 - band));
                }
                c = theme::lerp(c, t.red, cancel_k * 0.9);
                Span::styled("─".to_string(), Style::default().fg(t.c(c)))
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn input(frame: &mut Frame, area: Rect, app: &App, iv: &InputView) {
    if app.input.is_empty() {
        let t = &app.theme;
        let placeholder = if area.width >= MID {
            "ask anything  ·  / for commands"
        } else {
            "ask anything"
        };
        // The prompt breathes while the agent is busy.
        let pulse = if app.busy { breathe(app.now, 1000) } else { 1.0 };
        let prompt = theme::lerp(t.faint, t.accent, pulse);
        let line = Line::from(vec![
            Span::styled("› ".to_string(), Style::default().fg(t.c(prompt))),
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

pub fn status(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let w = area.width as usize;
    let s = &app.status;
    let now = app.now;

    let mut left: Vec<Span> = Vec::new();
    if app.busy {
        left.push(Span::styled(
            format!("{} ", anim::spinner(now)),
            Style::default().fg(t.c(t.accent)),
        ));
    } else {
        let dot = theme::lerp(t.faint, t.accent, breathe(now, 3000) * 0.35);
        left.push(Span::styled("· ", Style::default().fg(t.c(dot))));
    }
    left.push(Span::styled(s.model.clone(), Style::default().fg(t.c(t.dim))));

    // The thinking level rides next to the model, tinted by how hard it is
    // working, the way pi puts it on the right of its footer.
    let level_ink = theme::thinking_color(s.thinking);
    if s.thinking.is_off() {
        left.push(Span::styled(
            " • off".to_string(),
            Style::default().fg(t.fade(theme::THINK_OFF, 0.9)),
        ));
    } else {
        left.push(Span::styled(" • ".to_string(), Style::default().fg(t.c(t.faint))));
        left.push(Span::styled(
            s.thinking.as_str().to_string(),
            Style::default().fg(t.c(level_ink)).add_modifier(Modifier::BOLD),
        ));
    }

    // Cost flashes toward white when it changes, and every counter walks to
    // its new value instead of jumping.
    let flash = 1.0 - (now.saturating_sub(s.cost_flash) as f32 / 700.0).clamp(0.0, 1.0);
    let cost_color = theme::lerp(t.green, (255, 255, 255), flash * 0.7);
    let compact = w < WIDE as usize;

    let mut right: Vec<Span> = Vec::new();
    let show = |group: u8| -> bool {
        // Width tiers, so nothing has to be dropped after the fact.
        let room = w.saturating_sub(left.iter().map(|s| width(&s.content)).sum::<usize>() + 1);
        let needed = match group {
            3 => 34, // tokens + cache + meter + cost
            2 => 26, // tokens + meter + cost
            1 => 18, // meter + cost
            _ => 8,  // cost
        };
        room >= needed
    };
    if show(2) {
        right.push(Span::styled(
            format!(
                "↑{} ↓{}  ",
                short_num(s.disp_in(now)),
                short_num(s.disp_out(now))
            ),
            Style::default().fg(t.c(t.faint)),
        ));
    }
    // Cache traffic, when the provider reported any.
    if show(3) && (s.cache_read > 0 || s.cache_write > 0) {
        let hit = s.cache_hit_pct().unwrap_or(0);
        let color = if hit >= 50 { t.green } else { t.amber };
        right.push(Span::styled(
            format!("R{} {hit}%  ", short_num(s.cache_read)),
            Style::default().fg(t.c(color)),
        ));
    }
    let frac = s.ctx_frac(now);
    let pct = s.ctx_pct(now);
    if show(1) {
        let cells = if w < WIDE as usize { 4 } else { 6 };
        right.extend(ctx_meter(frac, cells, t));
        right.push(Span::raw(" ".to_string()));
        // Colour it before it becomes a problem, not after.
        let ctx_color = if pct > 90 {
            t.red
        } else if pct > 70 {
            t.amber
        } else {
            t.faint
        };
        right.push(Span::styled(
            format!("{pct}%  "),
            Style::default().fg(t.c(ctx_color)),
        ));
    }
    let (cost_text, cost_color) = match s.cost {
        // A model with no price in the table says so instead of showing a
        // zero it cannot back up.
        None => ("n/a".to_string(), t.faint),
        Some(_) => (fmt_cost(s.disp_cost(now), compact), cost_color),
    };
    right.push(Span::styled(
        format!("{cost_text}  "),
        Style::default().fg(t.c(cost_color)),
    ));

    let mut left_w: usize = left.iter().map(|s| width(&s.content)).sum();
    let right_w: usize = right.iter().map(|s| width(&s.content)).sum();
    // On a narrow phone the two halves can collide. The model label is the
    // part that gives way; the thinking level is one word and stays.
    if left_w + right_w + 1 > w {
        if let Some(model) = left.get_mut(1) {
            let room = w.saturating_sub(right_w + 2);
            let text = truncate(&model.content, room);
            model.content = text.into();
            left_w = left.iter().map(|s| width(&s.content)).sum();
        }
    }
    let pad = w.saturating_sub(left_w + right_w);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Filled blocks for the context budget, colored by how full it is.
fn ctx_meter(frac: f32, cells: usize, t: &theme::Theme) -> Vec<Span<'static>> {
    // Round up: any usage at all should light the first block, otherwise a
    // real 11% looks like an empty meter.
    let filled = ((frac * cells as f32).ceil() as usize).min(cells);
    let color = if frac > 0.8 {
        t.amber
    } else if frac > 0.5 {
        theme::lerp(t.accent, t.amber, (frac - 0.5) * 2.0)
    } else {
        t.accent
    };
    (0..cells)
        .map(|i| {
            let on = i < filled;
            Span::styled(
                if on { "▰" } else { "▱" }.to_string(),
                Style::default().fg(t.c(if on { color } else { t.faint })),
            )
        })
        .collect()
}

fn fmt_cost(cost: f64, compact: bool) -> String {
    if cost <= 0.0 {
        return "free".to_string();
    }
    if cost < 1.0 {
        let s = format!("{cost:.4}");
        if compact {
            // Drop the leading zero, terminals are narrow.
            return format!("${}", s.trim_start_matches('0'));
        }
        return format!("${s}");
    }
    format!("${cost:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorLevel, Theme};

    #[test]
    fn cost_formatting() {
        assert_eq!(fmt_cost(0.0, false), "free");
        assert_eq!(fmt_cost(0.0031, false), "$0.0031");
        assert_eq!(fmt_cost(0.0031, true), "$.0031");
        assert_eq!(fmt_cost(12.5, false), "$12.50");
    }

    #[test]
    fn meter_fills_with_fraction() {
        let t = Theme::forced(ColorLevel::True);
        assert_eq!(ctx_meter(0.0, 6, &t).len(), 6);
        let full: String = ctx_meter(1.0, 6, &t)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(full, "▰▰▰▰▰▰");
        let half: String = ctx_meter(0.5, 6, &t)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(half, "▰▰▰▱▱▱");
    }
}
