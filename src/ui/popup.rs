//! Slash-command palette. Floats above the composer, slides in, filters as
//! you type.

use crate::anim::{self, Tween};
use crate::app::{App, Popup};
use crate::textutil::{truncate, width};
use crate::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

pub fn draw(frame: &mut Frame, screen: Rect, input_area: Rect, app: &App, p: &Popup) {
    let t = &app.theme;
    let matches = p.matches();
    let rows = matches.len().min(6).max(1) as u16;
    let w = screen.width.saturating_sub(2).min(54).max(20);
    let h = rows + 2;
    // Slight overshoot on the way in, so the card feels like it snaps open.
    let t_in = Tween::new(220, anim::Ease::OutBack).t(app.now, p.opened_at);
    let slide = ((1.0 - t_in).max(0.0) * 2.0).round() as u16;

    // Sit clear of the rule above the composer, and wipe the column to the
    // left too so no message bar shows through the card's edge.
    let y_bottom = input_area.y.saturating_sub(1 + slide);
    let y = y_bottom.saturating_sub(h).max(screen.y);
    let x = screen.x + 1;
    let rect = Rect {
        x,
        y,
        width: w.min(screen.width.saturating_sub(1)),
        height: (y_bottom - y).min(h),
    };
    if rect.height < 3 {
        return;
    }

    let wipe = Rect {
        x: screen.x,
        y: rect.y,
        width: (rect.width + 1).min(screen.width),
        height: rect.height,
    };
    frame.render_widget(Clear, wipe);
    let border_color = theme::lerp(t.faint, t.accent, t_in);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.fade(border_color, t_in)))
        .style(Style::default().bg(t.c(t.card_bg)))
        .title(Line::from(Span::styled(
            " commands ",
            Style::default()
                .fg(t.fade(t.dim, t_in))
                .add_modifier(Modifier::BOLD),
        )))
        .title_bottom(
            Line::from(Span::styled(
                " tab to complete ",
                Style::default().fg(t.fade(t.faint, t_in)),
            ))
            .right_aligned(),
        )
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "no matching command",
            Style::default().fg(t.fade(t.faint, t_in)),
        )));
    }
    // The highlight eases between rows: it fades in on the new one and out on
    // the old one instead of teleporting.
    let sel_ease = Tween::new(170, anim::Ease::OutCubic).t(app.now, p.sel_at);

    for (i, cmd) in matches.iter().take(rows as usize).enumerate() {
        let hl = if i == p.sel {
            sel_ease
        } else if i == p.sel_prev {
            1.0 - sel_ease
        } else {
            0.0
        };
        let w_inner = inner.width as usize;
        let label = if cmd.args.is_empty() {
            format!("/{}", cmd.name)
        } else {
            format!("/{} {}", cmd.name, cmd.args)
        };
        let label_w = width(&label);
        let desc_budget = w_inner.saturating_sub(label_w + 4);
        let name_color = theme::lerp(t.text, t.accent, hl);
        let marker = if hl > 0.55 { "› " } else { "  " };
        let mut spans = vec![
            Span::styled(
                marker.to_string(),
                Style::default().fg(t.fade(t.accent, t_in * hl.max(0.15))),
            ),
            Span::styled(
                label,
                Style::default()
                    .fg(t.fade(name_color, t_in))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" ".to_string()),
        ];
        if desc_budget > 6 {
            spans.push(Span::styled(
                truncate(cmd.desc, desc_budget),
                Style::default().fg(t.fade(t.faint, t_in)),
            ));
        }
        let line = Line::from(spans);
        let line = if hl > 0.0 {
            let text_w: usize = line.spans.iter().map(|s| width(&s.content)).sum();
            let mut spans = line.spans;
            spans.push(Span::raw(" ".repeat(w_inner.saturating_sub(text_w))));
            Line::from(spans).style(Style::default().bg(t.fade(t.accent, 0.12 * t_in * hl)))
        } else {
            line
        };
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(ratatui::text::Text::from(lines)), inner);
}
