//! Key reference overlay. Termux users have no function keys, so this stays
//! reachable through /help.

use crate::anim::{self, Tween};
use crate::app::App;
use crate::textutil::width;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

const KEYS: &[(&str, &str)] = &[
    ("enter", "send"),
    ("ctrl+j", "new line"),
    ("tab", "complete command"),
    ("ctrl+p", "command palette"),
    ("up / down", "history"),
    ("pgup / pgdn", "scroll back"),
    ("ctrl+e", "expand last tool"),
    ("ctrl+w", "delete word"),
    ("ctrl+u", "clear input"),
    ("ctrl+c", "cancel, twice to quit"),
    ("ctrl+l", "redraw"),
    ("esc", "close"),
];

pub fn draw(frame: &mut Frame, screen: Rect, app: &App) {
    let t = &app.theme;
    let t_in = Tween::new(200, anim::Ease::OutCubic).t(app.now, app.help_at);
    let w = screen.width.saturating_sub(6).min(44).max(24);
    let h = (KEYS.len() as u16 + 4).min(screen.height.saturating_sub(2));
    let x = screen.x + (screen.width.saturating_sub(w)) / 2;
    let y = screen.y + (screen.height.saturating_sub(h)) / 2 + ((1.0 - t_in) * 2.0) as u16;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.fade(t.accent, t_in)))
        .style(Style::default().bg(t.c(t.card_bg)))
        .title(Line::from(Span::styled(
            " keys ",
            Style::default()
                .fg(t.fade(t.accent, t_in))
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let key_w = KEYS.iter().map(|(k, _)| width(k)).max().unwrap_or(8).max(6);
    let mut lines = Vec::new();
    for (k, d) in KEYS {
        if lines.len() as u16 >= inner.height {
            break;
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<key_w$}", k, key_w = key_w),
                Style::default()
                    .fg(t.fade(t.accent, t_in))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {d}"),
                Style::default().fg(t.fade(t.dim, t_in)),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(ratatui::text::Text::from(lines)), inner);
}
