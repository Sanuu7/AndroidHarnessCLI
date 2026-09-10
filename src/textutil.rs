//! Width-aware text helpers. Terminals are grids, phones are narrow, and
//! emoji/CJK are two cells wide, so nothing here touches `len()`.

use unicode_width::UnicodeWidthChar;

pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

pub fn width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Truncate to `max` cells, ending with an ellipsis when something was cut.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

pub fn pad_right(s: &str, to: usize) -> String {
    let mut out = s.to_string();
    let w = width(s);
    if w < to {
        out.push_str(&" ".repeat(to - w));
    }
    out
}

/// Greedy word wrap that respects explicit newlines. Never breaks a word
/// unless the word alone is wider than the line, in which case it hard-splits.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        wrap_line(raw, max, &mut out);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn wrap_line(line: &str, max: usize, out: &mut Vec<String>) {
    if line.is_empty() {
        out.push(String::new());
        return;
    }
    let mut cur = String::new();
    let mut cur_w = 0;
    for word in split_words(line) {
        let ww = width(&word);
        let is_space = word.chars().all(|c| c == ' ');
        if is_space {
            // Collapse runs of spaces down to one, unless they are leading
            // indentation on an otherwise empty line.
            if cur_w == 0 {
                let keep = ww.min(max);
                cur.push_str(&" ".repeat(keep.min(4)));
                cur_w += keep.min(4);
            } else if cur_w + 1 <= max {
                cur.push(' ');
                cur_w += 1;
            }
            continue;
        }
        if cur_w + ww <= max {
            cur.push_str(&word);
            cur_w += ww;
            continue;
        }
        if cur_w > 0 {
            out.push(std::mem::take(&mut cur));
        }
        if ww <= max {
            cur.push_str(&word);
            cur_w = ww;
        } else {
            // Word longer than the line: hard split on cell boundaries.
            let mut chunk = String::new();
            let mut cw = 0;
            for c in word.chars() {
                let w = char_width(c);
                if cw + w > max {
                    out.push(std::mem::take(&mut chunk));
                    cw = 0;
                }
                chunk.push(c);
                cw += w;
            }
            cur = chunk;
            cur_w = cw;
        }
    }
    out.push(cur);
}

fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_space = None;
    for c in line.chars() {
        let sp = c == ' ';
        if Some(sp) != in_space && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        in_space = Some(sp);
        cur.push(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// "1.2k" / "3.4M" style compaction for token counts.
pub fn short_num(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        let k = n as f64 / 1000.0;
        return if k < 10.0 {
            format!("{k:.1}k").replace(".0k", "k")
        } else {
            format!("{k:.0}k")
        };
    }
    let m = n as f64 / 1_000_000.0;
    format!("{m:.1}M").replace(".0M", "M")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_width() {
        assert_eq!(width("hello"), 5);
    }

    #[test]
    fn cjk_is_double_cell() {
        assert_eq!(width("中文"), 4);
        assert_eq!(width("café"), 4);
    }

    #[test]
    fn truncate_keeps_budget() {
        assert_eq!(truncate("hello world", 5), "hell…");
        assert!(width(&truncate("中文字符", 5)) <= 5);
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("x", 0), "");
    }

    #[test]
    fn wrap_respects_newlines_and_words() {
        let out = wrap("one two three\nfour", 7);
        assert_eq!(out, vec!["one two", "three", "four"]);
    }

    #[test]
    fn wrap_hard_splits_long_words() {
        let out = wrap("abcdefghij", 4);
        assert_eq!(out, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_preserves_blank_lines() {
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn wrap_is_width_safe_for_cjk() {
        for line in wrap("中文中文中文", 5) {
            assert!(width(&line) <= 5, "line too wide: {line}");
        }
    }

    #[test]
    fn pad_right_fills_to_width() {
        assert_eq!(pad_right("ab", 4), "ab  ");
        assert_eq!(width(&pad_right("中", 4)), 4);
    }

    #[test]
    fn short_num_formats() {
        assert_eq!(short_num(999), "999");
        assert_eq!(short_num(1200), "1.2k");
        assert_eq!(short_num(6200), "6.2k");
        assert_eq!(short_num(128_000), "128k");
        assert_eq!(short_num(1_500_000), "1.5M");
    }
}
