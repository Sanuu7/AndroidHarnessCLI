//! Boot animation: the wordmark builds up letter by letter with a band of
//! light travelling through it, a rule draws itself underneath, the tagline
//! fades in, then everything fades out as the chat takes over.
//!
//! Timeline (fractions of app::SPLASH_MS):
//!   0.10 - 0.60  letters appear, staggered
//!   0.30 - 0.70  rule expands
//!   0.50 - 0.80  tagline fades in
//!   0.82 - 1.00  fade out

use crate::anim::{self, Ease};
use crate::app::{App, SPLASH_MS};
use crate::theme::{self, lerp};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

const WORD: &str = "harness";

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let elapsed = app.now.saturating_sub(app.boot_at);
    let p = (elapsed as f32 / SPLASH_MS as f32).clamp(0.0, 1.0);

    let fade_out = {
        let k = ((p - 0.82) / 0.18).clamp(0.0, 1.0);
        1.0 - anim::ease(Ease::OutCubic, k)
    };

    let letters: Vec<char> = WORD.chars().collect();
    let n = letters.len();
    let band = anim::saw(app.now, 1800);

    let mut spans: Vec<Span> = Vec::new();
    for (i, ch) in letters.iter().enumerate() {
        let start = 0.10 + 0.50 * (i as f32 / n as f32);
        let k = ((p - start) / 0.22).clamp(0.0, 1.0);
        let appear = anim::ease(Ease::OutCubic, k);
        let base = theme::grad(t.accent, t.accent2, i as f32 / (n - 1) as f32);
        // Band of light travelling left to right through the letters.
        let lit = lerp(base, (255, 255, 255), anim::bell(i as f32, band * n as f32, 1.4) * 0.45);
        spans.push(Span::styled(
            ch.to_string(),
            Style::default()
                .fg(t.fade(lit, appear * fade_out))
                .add_modifier(Modifier::BOLD),
        ));
    }

    let rule_w = {
        let k = ((p - 0.30) / 0.40).clamp(0.0, 1.0);
        (anim::ease(Ease::OutQuint, k) * 22.0).round() as usize
    };
    let mut rule_spans: Vec<Span> = Vec::new();
    for i in 0..rule_w {
        let f = i as f32 / 21.0;
        let c = theme::grad(t.accent2, t.accent, f);
        rule_spans.push(Span::styled(
            "─".to_string(),
            Style::default().fg(t.fade(c, fade_out)),
        ));
    }

    let tagline_k = ((p - 0.50) / 0.30).clamp(0.0, 1.0);
    let tagline = Span::styled(
        "agent for the terminal".to_string(),
        Style::default().fg(t.fade(t.dim, anim::ease(Ease::InOutSine, tagline_k) * fade_out)),
    );

    let hint_k = ((p - 0.55) / 0.30).clamp(0.0, 1.0);
    let hint = Span::styled(
        "press any key".to_string(),
        Style::default().fg(t.fade(t.faint, anim::ease(Ease::Linear, hint_k) * 0.7 * fade_out)),
    );

    let cy = area.y + area.height / 2;
    let word_area = Rect {
        x: area.x,
        y: cy.saturating_sub(2),
        width: area.width,
        height: 1,
    };
    let rule_area = Rect {
        x: area.x,
        y: cy.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let tag_area = Rect {
        x: area.x,
        y: cy + 1,
        width: area.width,
        height: 1,
    };
    let hint_area = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(2),
        width: area.width,
        height: 1,
    };

    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        word_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(rule_spans)).alignment(Alignment::Center),
        rule_area,
    );
    frame.render_widget(Paragraph::new(Line::from(tagline)).alignment(Alignment::Center), tag_area);
    frame.render_widget(Paragraph::new(Line::from(hint)).alignment(Alignment::Center), hint_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Phase;
    use crate::theme::{ColorLevel, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn frame_text(app: &App, w: u16, h: u16) -> String {
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

    fn cell_at(app: &App, w: u16, h: u16, x: u16, y: u16) -> (String, ratatui::style::Color) {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, f.area(), app)).unwrap();
        match term.backend().buffer().cell((x, y)) {
            Some(c) => (
                c.symbol().to_string(),
                c.style().fg.unwrap_or(ratatui::style::Color::Reset),
            ),
            None => (String::new(), ratatui::style::Color::Reset),
        }
    }

    /// Column holding `needle` on a row, if the row draws it.
    fn find_x(app: &App, w: u16, h: u16, y: u16, needle: char) -> Option<u16> {
        (0..w).find(|x| cell_at(app, w, h, *x, y).0 == needle.to_string())
    }

    #[test]
    fn wordmark_builds_up_and_fades() {
        let mut app = App::new(Theme::forced(ColorLevel::True));
        app.phase = Phase::Splash;
        let (w, h) = (40u16, 12u16);
        let (word_row, rule_row, tag_row) = (h / 2 - 2, h / 2 - 1, h / 2 + 1);
        let bg = app.theme.c(app.theme.bg);

        app.now = 0;
        let x = find_x(&app, w, h, word_row, 'h').expect("wordmark holds its space");
        let dim = cell_at(&app, w, h, x, word_row).1;
        assert_eq!(dim, bg, "letters start invisible");
        let tag_x = find_x(&app, w, h, tag_row, 'a').expect("tagline holds its space");
        assert_eq!(cell_at(&app, w, h, tag_x, tag_row).1, bg, "tagline too early");
        assert!(find_x(&app, w, h, rule_row, '─').is_none(), "rule too early");

        app.now = 900;
        assert_ne!(
            cell_at(&app, w, h, x, word_row).1,
            bg,
            "letters should brighten as they arrive"
        );
        assert_ne!(
            cell_at(&app, w, h, tag_x, tag_row).1,
            bg,
            "tagline should be visible"
        );
        assert!(
            find_x(&app, w, h, rule_row, '─').is_some(),
            "rule should have drawn"
        );
        let mid = frame_text(&app, w, h);
        assert!(mid.contains("harness"));

        // By the end everything has faded back into the background.
        app.now = SPLASH_MS;
        assert_eq!(cell_at(&app, w, h, x, word_row).1, bg, "wordmark should fade out");
        assert_eq!(cell_at(&app, w, h, tag_x, tag_row).1, bg, "tagline should fade out");
    }
}
