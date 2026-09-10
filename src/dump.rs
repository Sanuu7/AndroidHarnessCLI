//! Headless frame dump.
//!
//! `harness --dump <state> [w] [h] [t_ms]` renders one frame at a chosen
//! moment into ANSI text on stdout. It is how the UI gets reviewed (and
//! screenshotted) without a terminal to drive.

use crate::app::{App, Item, Phase, Popup, ToolState};
use crate::theme::{ColorLevel, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier};
use std::io::{self, Write};

pub const STATES: &[&str] = &[
    "splash", "empty", "idle", "chat", "stream", "thinking", "tools", "error", "settle", "popup",
    "help",
];

pub fn run(args: &[String]) -> io::Result<()> {
    let state = args.first().map(|s| s.as_str()).unwrap_or("chat");
    let w: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(48);
    let h: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(36);
    let t: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(4_200);
    let level = if args.iter().any(|a| a == "--256") {
        ColorLevel::Indexed
    } else {
        ColorLevel::True
    };

    let app = build(state, level, t);

    let mut term = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    term.draw(|f| app.draw(f)).expect("draw");
    let buf = term.backend().buffer().clone();
    let out = to_ansi(&buf);
    let mut stdout = io::stdout();
    stdout.write_all(out.as_bytes())?;
    stdout.flush()
}

/// Usage numbers, already settled by the time this frame is taken.
fn usage(app: &mut App, tokens_in: u64, tokens_out: u64, cost: f64, t: u64) {
    app.status.set_usage(tokens_in, tokens_out, cost, t.saturating_sub(900));
    app.status.cost_flash = t.saturating_sub(400);
}

fn build(state: &str, level: ColorLevel, t: u64) -> App {
    let mut app = App::new(Theme::forced(level));
    app.boot_at = 0;
    app.chat_at = 0;
    app.now = t;
    app.status.model = "glm-5.3-flash".into();
    app.status.workspace = "AndroidHarness".into();

    match state {
        "splash" => {
            app.phase = Phase::Splash;
        }
        "empty" => {
            app.phase = Phase::Chat;
        }
        "idle" => {
            app.phase = Phase::Chat;
            app.items = vec![Item::Assistant {
                text: "Ready. What are we working on?".into(),
                born: 0,
                streaming: false,
                finished: 100,
            }];
        }
        "chat" => {
            app.phase = Phase::Chat;
            app.items = vec![
                Item::User {
                    text: "fix the token expiry off-by-one".into(),
                    born: 0,
                },
                Item::Assistant {
                    text: "Found it. The check uses `<=` where it should use `<`, so an expired \
                           token validates for one more second.\n\n```diff\n- if (now <= expiry) return \
                           true\n+ if (now < expiry) return true\n```\n\nApplied, tested, suite is \
                           green."
                        .into(),
                    born: 400,
                    streaming: false,
                    finished: 900,
                },
                Item::Tool {
                    name: "bash".into(),
                    args: "gradle :app:testDebugUnitTest".into(),
                    state: ToolState::Done {
                        ok: true,
                        ms: 8_400,
                        output: "3 tests completed, 3 passed".into(),
                        at: 1_000,
                    },
                    born: 1_000,
                    expanded: false,
                    expand_at: 0,
                },
            ];
            usage(&mut app, 12_480, 1_164, 0.0031, t);
        }
        "stream" => {
            app.phase = Phase::Chat;
            app.busy = true;
            app.items = vec![
                Item::User {
                    text: "explain where startup time goes".into(),
                    born: 0,
                },
                Item::Assistant {
                    text: "The app spends most of its startup budget on three things.\n\n## 1. \
                           Catalog parsing\n\nmodels.dev is 2.4 MB of JSON parsed on every cold \
                           start. Caching the parsed form cuts first paint in half."
                        .into(),
                    born: t.saturating_sub(700),
                    streaming: true,
                    finished: 0,
                },
            ];
            app.sent_at = t.saturating_sub(900);
            usage(&mut app, 6_200, 480, 0.0018, t);
        }
        "thinking" => {
            app.phase = Phase::Chat;
            app.busy = true;
            app.items = vec![
                Item::User {
                    text: "refactor the settings screen".into(),
                    born: 0,
                },
                Item::Thinking {
                    born: t.saturating_sub(1_300),
                },
            ];
            app.sent_at = t.saturating_sub(1_400);
            usage(&mut app, 5_100, 0, 0.0011, t);
        }
        "tools" => {
            app.phase = Phase::Chat;
            app.busy = true;
            app.items = vec![
                Item::User {
                    text: "run the doctor".into(),
                    born: 0,
                },
                Item::Tool {
                    name: "write_file".into(),
                    args: "doctor/unicode.txt".into(),
                    state: ToolState::Done {
                        ok: true,
                        ms: 6,
                        output: "wrote 34 bytes".into(),
                        at: 300,
                    },
                    born: 200,
                    expanded: false,
                    expand_at: 0,
                },
                Item::Tool {
                    name: "bash".into(),
                    args: "printf 'a\\na\\nunique\\na' | wc -c".into(),
                    state: ToolState::Done {
                        ok: true,
                        ms: 11,
                        output: "8".into(),
                        at: 500,
                    },
                    born: 400,
                    expanded: true,
                    expand_at: t.saturating_sub(600),
                },
                Item::Tool {
                    name: "web_fetch".into(),
                    args: "https://example.com".into(),
                    state: ToolState::Running {
                        started: t.saturating_sub(2_400),
                    },
                    born: 500,
                    expanded: false,
                    expand_at: 0,
                },
                Item::Note {
                    text: "6 of 6 checks passed".into(),
                    born: 600,
                    error: false,
                },
            ];
            usage(&mut app, 12_480, 1_164, 0.0031, t);
        }
        "error" => {
            app.phase = Phase::Chat;
            app.busy = true;
            app.items = vec![
                Item::User {
                    text: "run the test suite".into(),
                    born: 0,
                },
                Item::Tool {
                    name: "bash".into(),
                    args: "gradle :app:test".into(),
                    state: ToolState::Done {
                        ok: false,
                        ms: 3_200,
                        output: "FAILED: 2 tests, 1 failure\n  TokenValidatorTest.boundary".into(),
                        at: t.saturating_sub(120),
                    },
                    born: 200,
                    expanded: false,
                    expand_at: 0,
                },
                Item::Note {
                    text: "tool failed, stopping here".into(),
                    born: t.saturating_sub(80),
                    error: true,
                },
            ];
            usage(&mut app, 9_300, 620, 0.0024, t);
        }
        "settle" => {
            app.phase = Phase::Chat;
            app.items = vec![
                Item::User {
                    text: "summarize the last commit".into(),
                    born: 0,
                },
                Item::Assistant {
                    text: "The commit keyed the message loader so a send no longer remeasures the \
                           list mid-scroll."
                        .into(),
                    born: 200,
                    streaming: false,
                    finished: t.saturating_sub(260),
                },
            ];
            usage(&mut app, 7_400, 210, 0.0019, t);
        }
        "popup" => {
            app.phase = Phase::Chat;
            app.items = vec![Item::User {
                text: "compact the context".into(),
                born: 0,
            }];
            app.input.set_text("/");
            let mut popup = Popup::new(String::new(), t.saturating_sub(900));
            popup.select(1, t.saturating_sub(120));
            app.popup = Some(popup);
        }
        "help" => {
            app.phase = Phase::Chat;
            app.items = vec![Item::User {
                text: "how do i scroll".into(),
                born: 0,
            }];
            app.help = true;
            app.help_at = t.saturating_sub(900);
        }
        other => {
            eprintln!("unknown dump state: {other}");
            eprintln!("states: {}", STATES.join(" "));
        }
    }
    app
}

/// Serialize a ratatui buffer as ANSI, one sequence per style change.
pub fn to_ansi(buf: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut last: Option<String> = None;
        for x in 0..buf.area.width {
            let Some(cell) = buf.cell((x, y)) else { continue };
            let sgr = sgr_for(cell.style());
            if last.as_deref() != Some(sgr.as_str()) {
                out.push_str(&sgr);
                last = Some(sgr);
            }
            out.push_str(cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

fn sgr_for(style: ratatui::style::Style) -> String {
    let mut parts: Vec<String> = vec!["0".into()];
    let m = style.add_modifier;
    if m.contains(Modifier::BOLD) {
        parts.push("1".into());
    }
    if m.contains(Modifier::DIM) {
        parts.push("2".into());
    }
    if m.contains(Modifier::ITALIC) {
        parts.push("3".into());
    }
    if m.contains(Modifier::UNDERLINED) {
        parts.push("4".into());
    }
    if m.contains(Modifier::REVERSED) {
        parts.push("7".into());
    }
    if m.contains(Modifier::CROSSED_OUT) {
        parts.push("9".into());
    }
    if let Some(fg) = style.fg {
        parts.push(color_sgr(fg, false));
    }
    if let Some(bg) = style.bg {
        parts.push(color_sgr(bg, true));
    }
    format!("\x1b[{}m", parts.join(";"))
}

fn color_sgr(color: Color, bg: bool) -> String {
    let base = if bg { 40 } else { 30 };
    match color {
        Color::Rgb(r, g, b) => format!("{};2;{};{};{}", base + 8, r, g, b),
        Color::Indexed(i) => format!("{};5;{}", base + 8, i),
        Color::Black => format!("{}", base),
        Color::Red => format!("{}", base + 1),
        Color::Green => format!("{}", base + 2),
        Color::Yellow => format!("{}", base + 3),
        Color::Blue => format!("{}", base + 4),
        Color::Magenta => format!("{}", base + 5),
        Color::Cyan => format!("{}", base + 6),
        Color::Gray | Color::White => format!("{}", base + 7),
        Color::DarkGray => "90".to_string(),
        Color::LightRed => "91".to_string(),
        Color::LightGreen => "92".to_string(),
        Color::LightYellow => "93".to_string(),
        Color::LightBlue => "94".to_string(),
        Color::LightMagenta => "95".to_string(),
        Color::LightCyan => "96".to_string(),
        Color::Reset => "39".to_string(),
    }
}
