//! Screen composition. One place decides the layout, the views just fill it.

pub mod chat;
pub mod chrome;
pub mod help;
pub mod popup;
pub mod splash;

use crate::app::App;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

/// Phone-first column budget. Below these widths the chrome sheds pieces.
pub const WIDE: u16 = 60;
pub const MID: u16 = 42;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let iv = app
        .input
        .view(area.width as usize, 4, &app.theme, app.cursor_on(), true);
    let input_h = (iv.rows.len() as u16).clamp(1, 4);
    let show_header = area.height >= 14 && area.width >= 34;

    let mut constraints = Vec::new();
    if show_header {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(3));
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(input_h));
    constraints.push(Constraint::Length(1));
    let chunks = Layout::vertical(constraints).split(area);

    let mut i = 0;
    if show_header {
        chrome::header(frame, chunks[i], app);
        i += 1;
    }
    let transcript = chunks[i];
    let separator = chunks[i + 1];
    let input_area = chunks[i + 2];
    let status = chunks[i + 3];

    chat::draw(frame, transcript, app);
    chrome::separator(frame, separator, app);
    chrome::input(frame, input_area, app, &iv);
    chrome::status(frame, status, app);

    if let Some(p) = &app.popup {
        popup::draw(frame, area, input_area, app, p);
    }
    if app.help {
        help::draw(frame, area, app);
    }

    app.view_h.set(transcript.height);
    frame.set_cursor_position((
        input_area.x + iv.cursor.0,
        input_area.y + iv.cursor.1,
    ));
}
