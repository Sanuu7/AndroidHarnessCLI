//! Multiline composer.
//!
//! Chars are stored per logical line so cursor math stays honest around
//! multi-byte input. Display wrapping is derived on every draw by wrapping the
//! line prefix up to the cursor, which is exact because greedy wrapping is
//! prefix-stable.

use crate::textutil::{char_width, width, wrap};
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub const PROMPT: &str = "› ";
pub const PROMPT_W: usize = 2;

pub struct Input {
    pub lines: Vec<Vec<char>>,
    pub cx: usize,
    pub cy: usize,
    desired_x: Option<usize>,
    pub history: Vec<String>,
    hist_idx: Option<usize>,
    draft: Option<String>,
}

pub struct View {
    pub rows: Vec<Line<'static>>,
    /// Cursor position relative to the top-left of the rendered rows.
    pub cursor: (u16, u16),
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    pub fn new() -> Self {
        Self {
            lines: vec![Vec::new()],
            cx: 0,
            cy: 0,
            desired_x: None,
            history: Vec::new(),
            hist_idx: None,
            draft: None,
        }
    }

    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.is_empty())
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = text
            .split('\n')
            .map(|l| l.chars().collect::<Vec<_>>())
            .collect();
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.cy = self.lines.len() - 1;
        self.cx = self.lines[self.cy].len();
        self.desired_x = None;
    }

    pub fn clear(&mut self) {
        let history = std::mem::take(&mut self.history);
        *self = Input {
            history,
            ..Input::new()
        };
    }

    pub fn push_history(&mut self, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        if self.history.last().map(|h| h.as_str()) != Some(t) {
            self.history.push(t.to_string());
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        self.hist_idx = None;
        self.draft = None;
    }

    pub fn insert_char(&mut self, c: char) {
        self.lines[self.cy].insert(self.cx, c);
        self.cx += 1;
        self.desired_x = None;
    }

    /// Used by paste and completion, and by tests to build fixtures fast.
    #[allow(dead_code)]
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\n' => self.newline(),
                '\r' => {}
                _ => self.insert_char(c),
            }
        }
    }

    pub fn newline(&mut self) {
        let tail: Vec<char> = self.lines[self.cy].split_off(self.cx);
        self.lines.insert(self.cy + 1, tail);
        self.cy += 1;
        self.cx = 0;
        self.desired_x = None;
    }

    pub fn backspace(&mut self) {
        if self.cx > 0 {
            self.lines[self.cy].remove(self.cx - 1);
            self.cx -= 1;
        } else if self.cy > 0 {
            let line = self.lines.remove(self.cy);
            self.cy -= 1;
            self.cx = self.lines[self.cy].len();
            self.lines[self.cy].extend(line);
        }
        self.desired_x = None;
    }

    pub fn delete(&mut self) {
        if self.cx < self.lines[self.cy].len() {
            self.lines[self.cy].remove(self.cx);
        } else if self.cy + 1 < self.lines.len() {
            let next = self.lines.remove(self.cy + 1);
            self.lines[self.cy].extend(next);
        }
        self.desired_x = None;
    }

    /// Delete the word before the cursor, the usual readline behavior.
    pub fn kill_word(&mut self) {
        if self.cx == 0 {
            self.backspace();
            return;
        }
        while self.cx > 0 && self.lines[self.cy][self.cx - 1] == ' ' {
            self.backspace();
        }
        while self.cx > 0 && self.lines[self.cy][self.cx - 1] != ' ' {
            self.backspace();
        }
    }

    pub fn move_left(&mut self) {
        if self.cx > 0 {
            self.cx -= 1;
        } else if self.cy > 0 {
            self.cy -= 1;
            self.cx = self.lines[self.cy].len();
        }
        self.desired_x = None;
    }

    pub fn move_right(&mut self) {
        if self.cx < self.lines[self.cy].len() {
            self.cx += 1;
        } else if self.cy + 1 < self.lines.len() {
            self.cy += 1;
            self.cx = 0;
        }
        self.desired_x = None;
    }

    pub fn move_up(&mut self) -> bool {
        if self.cy > 0 {
            let want = *self.desired_x.get_or_insert(self.cx);
            self.cy -= 1;
            self.cx = want.min(self.lines[self.cy].len());
            true
        } else {
            false
        }
    }

    pub fn move_down(&mut self) -> bool {
        if self.cy + 1 < self.lines.len() {
            let want = self.desired_x.unwrap_or(self.cx);
            self.cy += 1;
            self.cx = want.min(self.lines[self.cy].len());
            true
        } else {
            false
        }
    }

    pub fn home(&mut self) {
        self.cx = 0;
        self.desired_x = None;
    }

    pub fn end(&mut self) {
        self.cx = self.lines[self.cy].len();
        self.desired_x = None;
    }

    /// Previous history entry. Returns true when the input changed.
    pub fn hist_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let next = match self.hist_idx {
            None => {
                self.draft = Some(self.text());
                self.history.len() - 1
            }
            Some(0) => return false,
            Some(i) => i - 1,
        };
        self.hist_idx = Some(next);
        let h = self.history[next].clone();
        self.set_text(&h);
        true
    }

    pub fn hist_next(&mut self) -> bool {
        let Some(i) = self.hist_idx else {
            return false;
        };
        if i + 1 < self.history.len() {
            self.hist_idx = Some(i + 1);
            let h = self.history[i + 1].clone();
            self.set_text(&h);
        } else {
            self.hist_idx = None;
            let d = self.draft.clone().unwrap_or_default();
            self.set_text(&d);
        }
        true
    }

    /// Wrap the input for display, bottom-anchored in `max_rows`.
    pub fn view(
        &self,
        width_total: usize,
        max_rows: usize,
        theme: &Theme,
        cursor_on: bool,
        focused: bool,
    ) -> View {
        let avail = width_total.saturating_sub(PROMPT_W).max(4);
        let mut rows: Vec<String> = Vec::new();
        let mut cursor_row = 0usize;
        let mut cursor_col = 0usize;

        for (i, line) in self.lines.iter().enumerate() {
            let s: String = line.iter().collect();
            let wrapped = wrap(&s, avail);
            if i == self.cy {
                let prefix: String = line[..self.cx.min(line.len())].iter().collect();
                let pre = wrap(&prefix, avail);
                cursor_row = rows.len() + pre.len() - 1;
                cursor_col = width(pre.last().map(|s| s.as_str()).unwrap_or(""));
            }
            rows.extend(wrapped);
        }

        let max_rows = max_rows.max(1);
        let mut off = rows.len().saturating_sub(max_rows);
        if cursor_row < off {
            off = cursor_row;
        }
        let cursor_y = cursor_row - off;

        let base = Style::default().fg(theme.c(theme.text));
        let prompt_style = Style::default().fg(theme.c(if focused { theme.accent } else { theme.faint }));

        let mut out: Vec<Line<'static>> = Vec::new();
        for (idx, row) in rows.iter().skip(off).take(max_rows).enumerate() {
            let prompt = if idx == 0 {
                Span::styled(PROMPT.to_string(), prompt_style)
            } else {
                Span::raw(" ".repeat(PROMPT_W))
            };
            let mut spans = vec![prompt];
            if idx == cursor_y && focused {
                let (head, cur, tail) = split_at_cell(row, cursor_col);
                spans.push(Span::styled(head, base));
                let cur_style = if cursor_on {
                    base.add_modifier(Modifier::REVERSED)
                } else {
                    base
                };
                spans.push(Span::styled(cur, cur_style));
                spans.push(Span::styled(tail, base));
            } else {
                spans.push(Span::styled(row.clone(), base));
            }
            out.push(Line::from(spans));
        }

        View {
            rows: out,
            cursor: (PROMPT_W as u16 + cursor_col as u16, cursor_y as u16),
        }
    }
}

/// Split a row into (before, cursor cell, after) at a cell column. When the
/// column is past the end of the row the cursor cell is a space.
fn split_at_cell(row: &str, col: usize) -> (String, String, String) {
    let mut acc = 0usize;
    for (byte_idx, c) in row.char_indices() {
        let w = char_width(c);
        if acc == col {
            let before = row[..byte_idx].to_string();
            let cur = c.to_string();
            let after = row[byte_idx + c.len_utf8()..].to_string();
            return (before, cur, after);
        }
        acc += w;
    }
    (row.to_string(), " ".to_string(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorLevel;

    fn input() -> Input {
        Input::new()
    }

    #[test]
    fn typing_and_deleting() {
        let mut i = input();
        i.insert_str("hello");
        assert_eq!(i.text(), "hello");
        i.backspace();
        assert_eq!(i.text(), "hell");
        i.move_left();
        i.delete();
        assert_eq!(i.text(), "hel");
    }

    #[test]
    fn newline_splits_and_joins() {
        let mut i = input();
        i.insert_str("ab");
        i.newline();
        i.insert_str("cd");
        assert_eq!(i.text(), "ab\ncd");
        assert_eq!((i.cx, i.cy), (2, 1));
        i.backspace();
        i.backspace();
        i.backspace();
        assert_eq!(i.text(), "ab");
        i.delete();
        assert_eq!(i.text(), "ab");
    }

    #[test]
    fn vertical_movement_keeps_column() {
        let mut i = input();
        i.set_text("long line here\nxxxx");
        assert_eq!((i.cx, i.cy), (4, 1));
        i.move_up();
        assert_eq!((i.cx, i.cy), (4, 0));
        i.move_down();
        assert_eq!((i.cx, i.cy), (4, 1));
        // Going up from the end of a longer line keeps the shorter line's end.
        i.move_up();
        i.end();
        assert_eq!(i.cx, 14);
        i.move_down();
        assert_eq!(i.cx, 4);
    }

    #[test]
    fn kill_word_removes_backwards() {
        let mut i = input();
        i.insert_str("one two three");
        i.kill_word();
        assert_eq!(i.text(), "one two ");
        i.kill_word();
        assert_eq!(i.text(), "one ");
    }

    #[test]
    fn history_round_trip() {
        let mut i = input();
        i.push_history("first");
        i.push_history("second");
        i.insert_str("draft");
        assert!(i.hist_prev());
        assert_eq!(i.text(), "second");
        assert!(i.hist_prev());
        assert_eq!(i.text(), "first");
        assert!(!i.hist_prev());
        assert!(i.hist_next());
        assert_eq!(i.text(), "second");
        assert!(i.hist_next());
        assert_eq!(i.text(), "draft");
    }

    #[test]
    fn cursor_tracks_wrapped_rows() {
        let mut i = input();
        i.insert_str("aaaa bbbb cccc dddd");
        let v = i.view(12, 5, &Theme::forced(ColorLevel::True), true, true);
        // avail = 10 cells, so this wraps to two rows with the cursor at the end.
        assert_eq!(v.rows.len(), 2);
        assert_eq!(v.cursor.1, 1);
        assert_eq!(v.cursor.0, 2 + 9);
        for line in &v.rows {
            let w: usize = line.spans.iter().map(|s| width(&s.content)).sum();
            assert!(w <= 12, "row too wide: {w}");
        }
    }

    #[test]
    fn view_windows_when_taller_than_area() {
        let mut i = input();
        for _ in 0..10 {
            i.insert_str("line\n");
        }
        let v = i.view(40, 3, &Theme::forced(ColorLevel::True), true, true);
        assert_eq!(v.rows.len(), 3);
        assert!(v.cursor.1 < 3);
    }

    #[test]
    fn unicode_cursor_is_char_indexed() {
        let mut i = input();
        i.insert_str("中文");
        assert_eq!(i.cx, 2);
        i.move_left();
        assert_eq!(i.cx, 1);
        let v = i.view(20, 3, &Theme::forced(ColorLevel::True), true, true);
        // 中文 is 4 cells, cursor after the first char sits at cell 2, plus prompt.
        assert_eq!(v.cursor.0, 2 + PROMPT_W as u16);
    }

    #[test]
    fn cursor_past_end_gets_a_cell() {
        let mut i = input();
        i.insert_str("ab");
        let v = i.view(20, 2, &Theme::forced(ColorLevel::True), true, true);
        let row: String = v.rows[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(row, "› ab ");
    }
}
