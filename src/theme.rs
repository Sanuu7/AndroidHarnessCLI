//! Palette and terminal capability handling.
//!
//! Everything here is a plain RGB triplet until the last moment, so animation
//! code can lerp colors without caring about the terminal's color support.
//! Conversion to indexed/16-color happens in [`Theme::c`].

use ratatui::style::Color;
use std::env;

pub type Rgb = (u8, u8, u8);

pub const TEXT: Rgb = (226, 232, 240);
pub const DIM: Rgb = (148, 163, 184);
pub const FAINT: Rgb = (94, 108, 128);

pub const ACCENT: Rgb = (94, 234, 212); // teal
pub const ACCENT2: Rgb = (167, 139, 250); // violet
pub const GREEN: Rgb = (74, 222, 128);
pub const RED: Rgb = (248, 113, 113);
pub const AMBER: Rgb = (251, 191, 36);

/// Message and card surfaces. Terminals render these over whatever background
/// the user has, so they are deliberately dark and low contrast.
pub const CODE_BG: Rgb = (26, 34, 48);
pub const CARD_BG: Rgb = (21, 27, 38);

/// A background for something the user said, so turns have anchors.
pub const USER_BG: Rgb = (34, 36, 48);

/// Tool cards are tinted by what happened, not just marked (pi's toolBg set).
pub const TOOL_PENDING_BG: Rgb = (30, 34, 44);
pub const TOOL_SUCCESS_BG: Rgb = (26, 38, 32);
pub const TOOL_ERROR_BG: Rgb = (44, 30, 32);

/// Markdown accents: headings warm, links cool, code in the accent.
pub const MD_HEADING: Rgb = (240, 198, 116);
pub const MD_LINK: Rgb = (129, 162, 190);

/// Reasoning levels get their own ramp so the footer and the thinking block
/// say how hard the model is working at a glance.
pub const THINK_OFF: Rgb = (88, 96, 110);
pub const THINK_MINIMAL: Rgb = (122, 128, 140);
pub const THINK_LOW: Rgb = (95, 135, 175);
pub const THINK_MEDIUM: Rgb = (129, 162, 190);
pub const THINK_HIGH: Rgb = (178, 148, 187);
pub const THINK_XHIGH: Rgb = (209, 131, 232);
pub const THINK_MAX: Rgb = (236, 120, 236);

pub fn thinking_color(level: crate::llm::Level) -> Rgb {
    use crate::llm::Level;
    match level {
        Level::Off => THINK_OFF,
        Level::Minimal => THINK_MINIMAL,
        Level::Low => THINK_LOW,
        Level::Medium => THINK_MEDIUM,
        Level::High => THINK_HIGH,
        Level::XHigh => THINK_XHIGH,
        Level::Max => THINK_MAX,
    }
}

/// One character per level, for the meter next to the model name.
pub fn thinking_ramp(level: crate::llm::Level) -> usize {
    use crate::llm::Level;
    match level {
        Level::Off => 0,
        Level::Minimal => 1,
        Level::Low => 2,
        Level::Medium => 3,
        Level::High => 4,
        Level::XHigh => 5,
        Level::Max => 6,
    }
}

/// Best guess at the terminal background, used to fade things "in" from
/// nothing. Override with HARNESS_BG=rrggbb when running on a light theme.
fn guess_bg() -> Rgb {
    if let Ok(v) = env::var("HARNESS_BG") {
        if let Some(rgb) = parse_hex(&v) {
            return rgb;
        }
    }
    (13, 17, 23)
}

fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColorLevel {
    True,
    Indexed,
    Basic,
}

impl ColorLevel {
    pub fn detect() -> Self {
        if let Ok(v) = env::var("HARNESS_COLOR") {
            return match v.as_str() {
                "true" | "truecolor" => ColorLevel::True,
                "256" | "indexed" => ColorLevel::Indexed,
                _ => ColorLevel::Basic,
            };
        }
        if let Ok(ct) = env::var("COLORTERM") {
            if ct.contains("truecolor") || ct.contains("24bit") {
                return ColorLevel::True;
            }
        }
        match env::var("TERM") {
            Ok(t) if t.contains("direct") || t.contains("kitty") => ColorLevel::True,
            Ok(t) if t.contains("256") => ColorLevel::Indexed,
            _ => ColorLevel::Basic,
        }
    }
}

pub struct Theme {
    pub level: ColorLevel,
    pub text: Rgb,
    pub dim: Rgb,
    pub faint: Rgb,
    pub accent: Rgb,
    pub accent2: Rgb,
    pub green: Rgb,
    pub red: Rgb,
    pub amber: Rgb,
    pub code_bg: Rgb,
    pub card_bg: Rgb,
    pub user_bg: Rgb,
    #[allow(dead_code)]
    pub tool_pending_bg: Rgb,
    #[allow(dead_code)]
    pub tool_success_bg: Rgb,
    #[allow(dead_code)]
    pub tool_error_bg: Rgb,
    pub md_heading: Rgb,
    pub md_link: Rgb,
    pub bg: Rgb,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            level: ColorLevel::detect(),
            text: TEXT,
            dim: DIM,
            faint: FAINT,
            accent: ACCENT,
            accent2: ACCENT2,
            green: GREEN,
            red: RED,
            amber: AMBER,
            code_bg: CODE_BG,
            card_bg: CARD_BG,
            user_bg: USER_BG,
            tool_pending_bg: TOOL_PENDING_BG,
            tool_success_bg: TOOL_SUCCESS_BG,
            tool_error_bg: TOOL_ERROR_BG,
            md_heading: MD_HEADING,
            md_link: MD_LINK,
            bg: guess_bg(),
        }
    }
}

impl Theme {
    /// Truecolor-friendly constructor for tests and frame dumps.
    pub fn forced(level: ColorLevel) -> Self {
        Theme {
            level,
            ..Default::default()
        }
    }

    pub fn c(&self, rgb: Rgb) -> Color {
        match self.level {
            ColorLevel::True => Color::Rgb(rgb.0, rgb.1, rgb.2),
            ColorLevel::Indexed => Color::Indexed(to_256(rgb)),
            ColorLevel::Basic => basicish(rgb),
        }
    }

    /// Color with alpha, faded toward the guessed background. `t` of 1 is the
    /// full color, 0 is invisible (background).
    pub fn fade(&self, rgb: Rgb, t: f32) -> Color {
        self.c(lerp(self.bg, rgb, t.clamp(0.0, 1.0)))
    }
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    (
        (a.0 as f32 + (b.0 as f32 - a.0 as f32) * t).round() as u8,
        (a.1 as f32 + (b.1 as f32 - a.1 as f32) * t).round() as u8,
        (a.2 as f32 + (b.2 as f32 - a.2 as f32) * t).round() as u8,
    )
}

/// Two-stop gradient, clamped at the ends.
pub fn grad(a: Rgb, b: Rgb, t: f32) -> Rgb {
    lerp(a, b, t)
}

/// Pick a point on a multi-stop ramp by normalized position.
pub fn ramp(stops: &[Rgb], t: f32) -> Rgb {
    if stops.is_empty() {
        return TEXT;
    }
    if stops.len() == 1 {
        return stops[0];
    }
    let t = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (t.floor() as usize).min(stops.len() - 2);
    grad(stops[i], stops[i + 1], t - i as f32)
}

fn to_256(rgb: Rgb) -> u8 {
    let (r, g, b) = rgb;
    let diff = (r as i32 - g as i32).abs().max((g as i32 - b as i32).abs()).max((r as i32 - b as i32).abs());
    let lum = (r as u32 + g as u32 + b as u32) / 3;
    // Map dark neutral / tinted-dark surface tones to the 232..255 grayscale ramp.
    // Otherwise a dark surface like (30, 34, 44) rounds to cube coordinate 1,1,1
    // which is index 59 (#5f5f5f, bright gray) and turns cards into light boxes.
    if lum < 95 && diff <= 24 {
        if lum < 8 {
            return 16;
        }
        let step = (((lum - 8) as f32) / 10.0).round() as u8;
        return 232 + step.min(23);
    }
    // Nearest point in the 6x6x6 xterm cube.
    let q = |v: u8| -> u16 { (v as u16 * 5 + 127) / 255 };
    (16 + 36 * q(rgb.0) + 6 * q(rgb.1) + q(rgb.2)) as u8
}

fn basicish(rgb: Rgb) -> Color {
    let (r, g, b) = (rgb.0 as i32, rgb.1 as i32, rgb.2 as i32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 40 {
        // Grayscale: split into black/gray/white by brightness.
        let lum = (r + g + b) / 3;
        return match lum {
            0..=60 => Color::DarkGray,
            61..=170 => Color::Gray,
            _ => Color::White,
        };
    }
    if r > g && r > b {
        if g > b + 40 { Color::Yellow } else { Color::Red }
    } else if g > r && g > b {
        if b > r + 40 { Color::Cyan } else { Color::Green }
    } else if r > g + 40 {
        Color::Magenta
    } else {
        Color::Blue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#ffffff"), Some((255, 255, 255)));
        assert_eq!(parse_hex("000000"), Some((0, 0, 0)));
        assert_eq!(parse_hex("nope"), None);
    }

    #[test]
    fn indexed_conversion_stays_in_range() {
        for rgb in [TEXT, ACCENT, RED, (0, 0, 0), (255, 255, 255), (30, 34, 44)] {
            let idx = to_256(rgb);
            assert!((16..=255).contains(&idx), "{} not in valid range", idx);
        }
    }

    #[test]
    fn lerp_endpoints() {
        assert_eq!(lerp((0, 0, 0), (255, 255, 255), 0.0), (0, 0, 0));
        assert_eq!(lerp((0, 0, 0), (255, 255, 255), 1.0), (255, 255, 255));
        assert_eq!(lerp((0, 0, 0), (255, 255, 255), 0.5), (128, 128, 128));
    }
}
