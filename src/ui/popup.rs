//! The card above the composer: the command palette, or a list to pick from.
//!
//! Both are the same widget with different rows, so the slide-in, the easing
//! highlight, and the shape stay identical.

use crate::anim::{self, Tween};
use crate::app::{App, Pick, Popup};
use crate::textutil::{truncate, width};
use crate::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

/// Rows visible at once. Enough for a phone, few enough to stay a card.
const MAX_ROWS: usize = 8;

pub fn draw(frame: &mut Frame, screen: Rect, input_area: Rect, app: &App, p: &Popup) {
    let t = &app.theme;
    let rows = p.rows().min(MAX_ROWS).max(1);
    let w = screen.width.saturating_sub(2).min(54).max(20);
    let h = rows as u16 + 2;
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
    let title = match p.pick {
        None => " commands ".to_string(),
        Some(pick) => format!(" {} ", pick.title()),
    };
    let hint = match p.pick {
        None => " tab to complete ".to_string(),
        Some(_) if p.loading => " fetching the list ".to_string(),
        Some(pick) => format!(" {} ", pick.hint()),
    };
    // Both sit on the border, so they have to fit inside it.
    let title_budget = rect.width.saturating_sub(6) as usize;
    let title = truncate(&title, title_budget.max(4));
    let hint = truncate(&hint, title_budget.saturating_sub(width(&title)).max(4));
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.fade(border_color, t_in)))
        .style(Style::default().bg(t.c(t.card_bg)))
        .title(Line::from(Span::styled(
            title,
            Style::default()
                .fg(t.fade(t.dim, t_in))
                .add_modifier(Modifier::BOLD),
        )))
        .title_bottom(
            Line::from(Span::styled(hint, Style::default().fg(t.fade(t.faint, t_in))))
                .right_aligned(),
        )
        .padding(Padding::horizontal(1));

    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let w_inner = inner.width as usize;
    let sel_ease = Tween::new(170, anim::Ease::OutCubic).t(app.now, p.sel_at);
    let mut lines: Vec<Line> = Vec::new();

    match p.pick {
        None => {
            let matches = p.commands_matching();
            if matches.is_empty() {
                lines.push(empty_line(t, t_in));
            }
            // The list scrolls with the highlight so the selection is always
            // on screen, which matters when the palette is filtered.
            let start = scroll_start(p.sel, matches.len(), MAX_ROWS);
            for (i, cmd) in matches.iter().skip(start).take(MAX_ROWS).enumerate() {
                let index = start + i;
                let hl = highlight(index, p.sel, p.sel_prev, sel_ease);
                let label = if cmd.args.is_empty() {
                    format!("/{}", cmd.name)
                } else {
                    format!("/{} {}", cmd.name, cmd.args)
                };
                lines.push(row(t, t_in, hl, &label, cmd.desc, w_inner));
            }
        }
        Some(pick) => {
            let visible = p.visible();
            if visible.is_empty() {
                if p.filter.trim().is_empty() {
                    lines.push(empty_line(t, t_in));
                } else if pick == Pick::Model {
                    // Nothing matched: the typed text becomes the model name.
                    lines.push(row(
                        t,
                        t_in,
                        1.0,
                        &format!("use \"{}\"", p.filter.trim()),
                        "not in the list yet",
                        w_inner,
                    ));
                } else {
                    lines.push(empty_line(t, t_in));
                }
            }
            let start = scroll_start(p.sel, visible.len(), MAX_ROWS);
            for (i, choice) in visible.iter().skip(start).take(MAX_ROWS).enumerate() {
                let index = start + i;
                let hl = highlight(index, p.sel, p.sel_prev, sel_ease);
                let label = if choice.current {
                    format!("{} ✓", choice.label)
                } else {
                    choice.label.clone()
                };
                lines.push(row(t, t_in, hl, &label, &choice.hint, w_inner));
            }
        }
    }
    frame.render_widget(Paragraph::new(ratatui::text::Text::from(lines)), inner);
}

fn empty_line(t: &theme::Theme, t_in: f32) -> Line<'static> {
    Line::from(Span::styled(
        "nothing matches",
        Style::default().fg(t.fade(t.faint, t_in)),
    ))
}

/// Which row the highlight sits on, cross-fading from the previous one.
fn highlight(index: usize, sel: usize, prev: usize, ease: f32) -> f32 {
    if index == sel {
        ease
    } else if index == prev {
        1.0 - ease
    } else {
        0.0
    }
}

/// First row to draw so the highlight stays inside the window.
fn scroll_start(sel: usize, total: usize, window: usize) -> usize {
    if total <= window {
        return 0;
    }
    let ideal = sel.saturating_sub(window / 2);
    ideal.min(total - window)
}

fn row(
    t: &theme::Theme,
    t_in: f32,
    hl: f32,
    label: &str,
    hint: &str,
    w_inner: usize,
) -> Line<'static> {
    let label_w = width(label);
    let hint_budget = w_inner.saturating_sub(label_w + 4);
    let name_color = theme::lerp(t.text, t.accent, hl);
    let marker = if hl > 0.55 { "› " } else { "  " };
    let mut spans = vec![
        Span::styled(
            marker.to_string(),
            Style::default().fg(t.fade(t.accent, t_in * hl.max(0.15))),
        ),
        Span::styled(
            label.to_string(),
            Style::default()
                .fg(t.fade(name_color, t_in))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if hint_budget > 5 && !hint.is_empty() {
        spans.push(Span::raw(" ".to_string()));
        spans.push(Span::styled(
            truncate(hint, hint_budget),
            Style::default().fg(t.fade(t.faint, t_in)),
        ));
    }
    let line = Line::from(spans);
    if hl <= 0.0 {
        return line;
    }
    let text_w: usize = line.spans.iter().map(|s| width(&s.content)).sum();
    let mut spans = line.spans;
    spans.push(Span::raw(" ".repeat(w_inner.saturating_sub(text_w))));
    Line::from(spans).style(Style::default().bg(t.fade(t.accent, 0.12 * t_in * hl)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_follows_the_highlight() {
        assert_eq!(scroll_start(0, 3, 8), 0);
        assert_eq!(scroll_start(0, 20, 8), 0);
        assert_eq!(scroll_start(19, 20, 8), 12);
        assert_eq!(scroll_start(10, 20, 8), 6);
    }

    #[test]
    fn highlights_cross_fade_between_rows() {
        assert_eq!(highlight(2, 2, 5, 0.5), 0.5);
        assert_eq!(highlight(5, 2, 5, 0.5), 0.5);
        assert_eq!(highlight(1, 2, 5, 0.5), 0.0);
    }
}
