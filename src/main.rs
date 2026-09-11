//! harness: an agent for the terminal, built for a phone in your pocket.

mod agent;
mod anim;
mod app;
mod config;
mod diff;
mod dump;
mod input;
mod llm;
mod markdown;
mod session;
mod textutil;
mod theme;
mod tools;
mod ui;

use agent::Agent;
use app::App;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use theme::Theme;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut options = Options::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--dump" => return dump::run(&args[i + 1..]),
            "--print" => {
                i += 1;
                options.print = Some(args.get(i).cloned().unwrap_or_default());
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            "--version" | "-V" => {
                println!("harness {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--model" | "-m" => {
                i += 1;
                options.model = args.get(i).cloned();
            }
            "--thinking" | "-t" => {
                i += 1;
                options.thinking = args.get(i).cloned();
            }
            "--provider" | "-p" => {
                i += 1;
                options.provider = args.get(i).cloned();
            }
            "--dir" | "-C" => {
                i += 1;
                options.dir = args.get(i).cloned();
            }
            "--continue" | "-c" => {
                options.resume = Some(args.get(i + 1).filter(|a| !a.starts_with('-')).cloned());
                if matches!(options.resume, Some(Some(_))) {
                    i += 1;
                }
            }
            other => {
                eprintln!("harness: unknown argument '{other}'");
                eprintln!("try: harness --help");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if let Some(prompt) = options.print.clone() {
        run_print(&prompt, options)?;
        return Ok(());
    }
    run_tui(options)
}

#[derive(Default)]
struct Options {
    model: Option<String>,
    provider: Option<String>,
    dir: Option<String>,
    /// One-shot, headless: prompt in, answer on stdout.
    print: Option<String>,
    /// Thinking level for this run, overriding the config.
    thinking: Option<String>,
    /// Some(None) resumes the newest session, Some(Some(id)) a specific one.
    resume: Option<Option<String>>,
}

fn print_usage() {
    println!(
        "harness {}

An agent for the terminal, built for a phone. No key needed to start.

usage: harness [options]

  -m, --model <name>       use this model
  -t, --thinking <level>   off, minimal, low, medium, high, xhigh, max
  -p, --provider <name>    use this provider from the config
  -C, --dir <path>         work in this directory
  -c, --continue [id]      resume the newest session, or one by id
      --print <prompt>     answer once and exit, no interface (tool calls on stderr)
      --dump <state> [w] [h] [t_ms] [--256]
                           render one frame as ansi, for tests and screenshots
  -h, --help               this text
  -V, --version            print the version

keys:
  enter      send            ctrl+j     new line
  tab        complete        type /     commands and pickers
  up/down    history         shift+tab  cycle thinking level
  ctrl+t     show thinking   ctrl+o     expand tool output
  ctrl+e     expand tool     ctrl+s     save in a picker
  pgup/pgdn  scroll back     ctrl+w     delete word
  ctrl+c     cancel, twice to quit      ctrl+d  quit now

commands: /model /provider /thinking /sessions /new /clear /compact /cost
          /doctor /init /skills /memory /todos /context /plan /stop /theme /help /quit

env:
  HARNESS_API_KEY=...         key for the active provider
  HARNESS_MODEL=...           override the model
  HARNESS_THINKING=...        off, minimal, low, medium, high, xhigh, max
  HARNESS_COLOR=true|256|16   force a color depth
  HARNESS_BG=rrggbb           terminal background guess, for fade-ins

config: {}/config.json  (providers and keys, 0600)",
        env!("CARGO_PKG_VERSION"),
        config::Config::dir().display()
    );
}

fn run_tui(options: Options) -> io::Result<()> {
    if let Some(dir) = &options.dir {
        std::env::set_current_dir(dir)?;
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        hook(info);
    }));

    let mut app = App::new(Theme::default());
    if let Some((agent, cfg)) = build_agent(&mut app, &options) {
        app.attach(agent, cfg);
    }

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let start = Instant::now();
    let res = event_loop(&mut terminal, &mut app, start);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    res
}

/// Load settings, pick the provider, and start the worker.
fn build_agent(app: &mut App, options: &Options) -> Option<(Agent, config::Config)> {
    let mut cfg = config::Config::load();
    if let Some(name) = &options.provider {
        if cfg.find(name).is_none() {
            app.note(&format!("no provider named '{name}' in the config, using the default"), true);
        } else {
            cfg.provider = name.clone();
        }
    }
    if let Some(model) = &options.model {
        cfg.model = model.clone();
    }
    let saved = resume(&cfg, options).filter(|(session, _)| {
        let valid = session.matches_workspace(&std::env::current_dir().unwrap_or_default());
        if !valid { app.note(&format!("open {} to resume this session", session.workspace.display()), true); }
        valid
    });
    if let Some((session, _)) = &saved {
        if options.provider.is_none() && cfg.find(&session.provider).is_some() {
            cfg.provider = session.provider.clone();
        }
        if options.model.is_none() && cfg.provider == session.provider && !session.model.is_empty() { cfg.model = session.model.clone(); }
    }
    if let Some(level) = options.thinking.as_deref().and_then(llm::Level::parse) { cfg.thinking = level; }
    let provider = cfg.active().clone();
    if provider.kind != config::Kind::Relay && provider.api_key.is_empty() {
        app.note(
            &format!(
                "{} has no api key yet. put it in {} or set HARNESS_API_KEY",
                provider.name,
                cfg.path.display()
            ),
            true,
        );
    }
    if !llm::http::curl_available() {
        app.note("curl is not installed, and the engine needs it. run: pkg install curl", true);
    }
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if !root.join(".harness").exists() {
        let _ = std::fs::create_dir_all(root.join(".harness/memory"));
    }
    app.status.branch = agent::git_branch(&root).unwrap_or_default();
    let ctx_max = agent::default_ctx_max(&cfg.model);
    let agent = Agent::start(provider, cfg.model.clone(), root, ctx_max, cfg.thinking, app.prefs.clone());

    match saved {
        Some((session, from_args)) => {
            app.restore_messages(&session);
            let messages = session.messages.len();
            let title = session.title.clone();
            let model = session.model.clone();
            app.note(&format!("resumed {title} ({messages} messages)"), false);
            if model != cfg.model && from_args {
                app.note(&format!("that session used {model}"), false);
            }
            agent.load(session);
        }
        None => {
            if let Some(Some(id)) = &options.resume {
                app.note(&format!("no session with id {id}"), true);
            }
        }
    }
    Some((agent, cfg))
}

fn resume(cfg: &config::Config, options: &Options) -> Option<(session::Session, bool)> {    let want = options.resume.as_ref()?;
    let path = match want {
        Some(id) => {
            let dir = session::Session::dir();
            let exact = dir.join(format!("{id}.json"));
            if exact.exists() {
                exact
            } else {
                session::Session::list()
                    .into_iter()
                    .find(|(sid, _, _, _)| sid.starts_with(id))
                    .map(|(_, _, _, path)| path)?
            }
        }
        None => session::Session::latest()?,
    };
    let session = session::Session::load(&path)?;
    let _ = cfg;
    Some((session, want.is_some()))
}

/// Headless: one prompt, the answer on stdout, progress on stderr. This is
/// also how the engine gets exercised without a terminal to drive.
fn run_print(prompt: &str, options: Options) -> io::Result<()> {
    use std::io::Write;
    if let Some(dir) = &options.dir {
        std::env::set_current_dir(dir)?;
    }
    let cfg = config::Config::load();
    let mut model = cfg.model.clone();
    if let Some(name) = &options.model {
        model = name.clone();
    }
    let mut provider_name = cfg.provider.clone();
    if let Some(name) = &options.provider {
        provider_name = name.clone();
    }
    let provider = match cfg.find(&provider_name) {
        Some(p) => p.clone(),
        None => {
            eprintln!("harness: no provider named '{provider_name}'");
            std::process::exit(2);
        }
    };
    if !llm::http::curl_available() {
        eprintln!("harness: curl is required and was not found on PATH");
        std::process::exit(2);
    }
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let ctx_max = agent::default_ctx_max(&model);
    let level = match options.thinking.as_deref().and_then(llm::Level::parse) {
        Some(level) => level,
        None => cfg.thinking,
    };
    let agent = Agent::start(provider, model, root, ctx_max, level, Vec::new());
    agent.say(prompt);

    let mut streamed = false;
    loop {
        for event in agent.poll() {
            match event {
                agent::AgentEvent::Token(t) => {
                    streamed = true;
                    print!("{t}");
                    io::stdout().flush()?;
                }
                agent::AgentEvent::Reasoning(t) => {
                    eprint!("\x1b[2m{t}\x1b[0m");
                }
                agent::AgentEvent::ToolStart { name, args } => {
                    if streamed {
                        println!();
                        streamed = false;
                    }
                    eprintln!("→ {name} {args}");
                }
                agent::AgentEvent::ToolDone { name, ok, ms, output } => {
                    let mark = if ok { "✓" } else { "✗" };
                    let first = output.lines().next().unwrap_or("").chars().take(120).collect::<String>();
                    eprintln!("{mark} {name} {ms}ms · {first}");
                }
                agent::AgentEvent::Notice(text) => eprintln!("· {text}"),
                agent::AgentEvent::Error(text) => {
                    eprintln!("harness: {text}");
                    std::process::exit(1);
                }
                agent::AgentEvent::Usage { tokens_in, tokens_out, cost, .. } => {
                    let cost = match cost {
                        Some(c) if c == 0.0 => "free".to_string(),
                        Some(c) => format!("${c:.4}"),
                        None => "unpriced".to_string(),
                    };
                    eprintln!("· {tokens_in} in / {tokens_out} out · {cost}");
                }
                agent::AgentEvent::Done => {
                    println!();
                    return Ok(());
                }
                agent::AgentEvent::Cancelled => {
                    eprintln!("harness: cancelled");
                    return Ok(());
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
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
                Event::Paste(text) => {
                    for line in text.lines() {
                        for c in line.chars() {
                            app.input.insert_char(c);
                        }
                        app.input.newline();
                    }
                }
                _ => {}
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}
