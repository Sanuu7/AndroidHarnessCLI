//! harness: an agent UI for the terminal, built for a phone in your pocket.
//!
//! This build is the interface layer. The scripts behind it are fake (see
//! `demo.rs`) so the motion and layout can be judged before the engine lands.

mod anim;
mod app;
mod demo;
mod dump;
mod input;
mod markdown;
mod textutil;
mod theme;
mod ui;

use app::App;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use std::io::{self, stdout};
use std::time::{Duration, Instant};
use theme::Theme;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("--dump") => return dump::run(&args[1..]),
        Some("--help" | "-h") => {
            print_usage();
            return Ok(());
        }
        Some("--version" | "-V") => {
            println!("harness {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {}
    }
    run_tui()
}

fn print_usage() {
    println!(
        "harness {}

usage: harness [--dump <state> [w] [h] [t_ms] [--256]]

  states: splash chat stream tools popup help idle

keys:
  enter      send            ctrl+j     new line
  tab        complete        ctrl+p     command palette
  up/down    history         pgup/pgdn  scroll back
  ctrl+e     expand tool     ctrl+w     delete word
  ctrl+c     cancel, twice to quit      ctrl+l  redraw

env:
  HARNESS_COLOR=true|256|16   force a color depth
  HARNESS_BG=rrggbb           terminal background guess, for fade-ins",
        env!("CARGO_PKG_VERSION")
    );
}

fn run_tui() -> io::Result<()> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        hook(info);
    }));

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(Theme::default());
    let start = Instant::now();

    let res = event_loop(&mut terminal, &mut app, start);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    start: Instant,
) -> io::Result<()> {
    loop {
        let now = start.elapsed().as_millis() as u64;
        app.update(now);

        if app.clear_screen {
            terminal.clear()?;
            app.clear_screen = false;
        }
        terminal.draw(|frame| app.draw(frame))?;

        let timeout = if app.is_animating() {
            Duration::from_millis(anim::FPS_BUSY_MS)
        } else {
            Duration::from_millis(anim::FPS_IDLE_MS)
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => app.on_key(key),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}
