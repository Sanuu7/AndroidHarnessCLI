//! Application state and the event loop's brain.
//!
//! Everything time-dependent reads `self.now`, which main updates each frame
//! from the monotonic clock. Frame dumps set it by hand so animations can be
//! inspected mid-flight.

use crate::agent::{Agent, AgentEvent};
use crate::anim::{self, Tween};
use crate::input::Input;
use crate::theme::Theme;
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::cell::Cell;
use std::sync::mpsc::{Receiver, TryRecvError};

pub const SPLASH_MS: u64 = 1500;

#[derive(PartialEq, Clone, Copy)]
pub enum Phase {
    Splash,
    Chat,
}

pub enum ToolState {
    Running {
        started: u64,
    },
    Done {
        ok: bool,
        ms: u64,
        output: String,
        /// When it landed, so the card can settle in and shake on failure.
        at: u64,
    },
}

pub enum Item {
    User {
        text: String,
        born: u64,
    },
    Assistant {
        text: String,
        born: u64,
        streaming: bool,
        finished: u64,
    },
    /// The model is working before its first visible token.
    Thinking {
        born: u64,
    },
    /// A reasoning stream: shown quiet and clamped, it is not the answer.
    Reasoning {
        text: String,
        born: u64,
    },
    Tool {
        name: String,
        args: String,
        state: ToolState,
        born: u64,
        expanded: bool,
        expand_at: u64,
    },
    Note {
        text: String,
        born: u64,
        error: bool,
    },
}

impl Item {
    pub fn born(&self) -> u64 {
        match self {
            Item::User { born, .. }
            | Item::Assistant { born, .. }
            | Item::Thinking { born }
            | Item::Reasoning { born, .. }
            | Item::Tool { born, .. }
            | Item::Note { born, .. } => *born,
        }
    }
}

pub struct Status {
    pub model: String,
    pub provider: String,
    pub workspace: String,
    pub session: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub ctx_max: u64,
    /// None when the model has no price in the table.
    pub cost: Option<f64>,
    pub cost_flash: u64,
    // Displayed values walk toward the real ones so counters never jump.
    anim_in: anim::Anim,
    anim_out: anim::Anim,
    anim_cost: anim::Anim,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            model: String::new(),
            provider: String::new(),
            workspace: std::env::current_dir()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| "~".into()),
            session: String::new(),
            tokens_in: 0,
            tokens_out: 0,
            ctx_max: 128_000,
            cost: Some(0.0),
            cost_flash: 0,
            anim_in: anim::Anim::at(0.0),
            anim_out: anim::Anim::at(0.0),
            anim_cost: anim::Anim::at(0.0),
        }
    }
}

impl Status {
    /// New usage numbers: targets update immediately, the readout catches up.
    pub fn set_usage(
        &mut self,
        tokens_in: u64,
        tokens_out: u64,
        cost: Option<f64>,
        now: u64,
    ) {
        if tokens_in != self.tokens_in {
            self.anim_in.set(tokens_in as f32, now, 520);
        }
        if tokens_out != self.tokens_out {
            self.anim_out.set(tokens_out as f32, now, 520);
        }
        let price = cost.map(|c| c as f32).unwrap_or(self.anim_cost.get(now));
        if cost.is_some() && (cost.unwrap() - self.cost.unwrap_or(-1.0)).abs() > f64::EPSILON {
            self.anim_cost.set(price, now, 520);
            self.cost_flash = now;
        }
        self.tokens_in = tokens_in;
        self.tokens_out = tokens_out;
        self.cost = cost;
    }

    pub fn disp_in(&self, now: u64) -> u64 {
        self.anim_in.get(now).round().max(0.0) as u64
    }

    pub fn disp_out(&self, now: u64) -> u64 {
        self.anim_out.get(now).round().max(0.0) as u64
    }

    pub fn disp_cost(&self, now: u64) -> f64 {
        self.anim_cost.get(now).max(0.0) as f64
    }

    /// Context fill as a fraction, used by the meter and the percentage.
    pub fn ctx_frac(&self, now: u64) -> f32 {
        if self.ctx_max == 0 {
            return 0.0;
        }
        ((self.anim_in.get(now) + self.anim_out.get(now)) / self.ctx_max as f32).clamp(0.0, 1.0)
    }

    pub fn ctx_pct(&self, now: u64) -> u32 {
        (self.ctx_frac(now) * 100.0).round() as u32
    }
}

pub struct Command {
    pub name: &'static str,
    pub args: &'static str,
    pub desc: &'static str,
}

pub const COMMANDS: &[Command] = &[
    Command { name: "clear", args: "", desc: "forget this conversation" },
    Command { name: "compact", args: "", desc: "summarize older messages" },
    Command { name: "cost", args: "", desc: "spend for this session" },
    Command { name: "doctor", args: "", desc: "self-test the tools" },
    Command { name: "help", args: "", desc: "keys and commands" },
    Command { name: "init", args: "", desc: "write AGENTS.md for this repo" },
    Command { name: "model", args: "[name]", desc: "switch the model" },
    Command { name: "new", args: "", desc: "start a fresh session" },
    Command { name: "provider", args: "[name]", desc: "switch or list providers" },
    Command { name: "quit", args: "", desc: "leave" },
    Command { name: "sessions", args: "", desc: "past sessions" },
    Command { name: "skills", args: "", desc: "list installed skills" },
    Command { name: "theme", args: "", desc: "match your terminal" },
];

pub struct Popup {
    pub filter: String,
    pub sel: usize,
    pub opened_at: u64,
    /// Where the highlight was before, so it can cross-fade between rows.
    pub sel_prev: usize,
    pub sel_at: u64,
}

impl Popup {
    pub fn new(filter: String, now: u64) -> Self {
        Self {
            filter,
            sel: 0,
            opened_at: now,
            sel_prev: 0,
            sel_at: now,
        }
    }

    /// Move the highlight, remembering where it came from.
    pub fn select(&mut self, sel: usize, now: u64) {
        if sel != self.sel {
            self.sel_prev = self.sel;
            self.sel = sel;
            self.sel_at = now;
        }
    }
}

impl Popup {
    pub fn matches(&self) -> Vec<&'static Command> {
        let f = self.filter.to_lowercase();
        COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(&f) || c.name.contains(&f))
            .collect()
    }
}

pub struct App {
    pub theme: Theme,
    pub now: u64,
    pub boot_at: u64,
    pub chat_at: u64,
    pub phase: Phase,
    pub items: Vec<Item>,
    pub input: Input,
    pub scroll: usize,
    pub busy: bool,
    pub popup: Option<Popup>,
    pub help: bool,
    pub help_at: u64,
    pub status: Status,
    pub quit: bool,
    pub clear_screen: bool,
    pub ctrl_c_at: u64,
    pub view_h: Cell<u16>,
    /// When the last message was sent, which lights up the composer.
    pub sent_at: u64,
    /// When a run was cancelled, which flashes the divider red.
    pub cancel_at: u64,
    /// The engine. Frame dumps run without one.
    agent: Option<Agent>,
    /// The settings file, for switching models and providers at runtime.
    pub config: Option<crate::config::Config>,
    /// Events from tool runs the UI started itself, such as /doctor.
    local: Option<Receiver<AgentEvent>>,
    /// Workspace preferences handed to the agent at startup.
    pub prefs: Vec<String>,
}

impl App {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            now: 0,
            boot_at: 0,
            chat_at: 0,
            phase: Phase::Splash,
            items: Vec::new(),
            input: Input::new(),
            scroll: 0,
            busy: false,
            popup: None,
            help: false,
            help_at: 0,
            status: Status::default(),
            quit: false,
            clear_screen: false,
            ctrl_c_at: 0,
            view_h: Cell::new(20),
            sent_at: 0,
            cancel_at: 0,
            agent: None,
            config: None,
            local: None,
            prefs: Vec::new(),
        }
    }

    /// Hand the app a live engine and the settings it came from.
    pub fn attach(&mut self, agent: Agent, config: crate::config::Config) {
        self.status.model = agent.model.clone();
        self.status.provider = agent.provider.clone();
        self.status.ctx_max = agent.ctx_max;
        self.agent = Some(agent);
        self.config = Some(config);
    }

    /// Nothing on screen yet, so the views show the welcome.
    pub fn is_empty_state(&self) -> bool {
        self.items.is_empty() && !self.busy
    }

    pub fn update(&mut self, now: u64) {
        self.now = now;
        if let Some(agent) = &self.agent {
            for ev in agent.poll() {
                self.on_agent(ev);
            }
        }
        // Local jobs (a UI-run tool) push through the same handler. The
        // receiver is taken out so the handler can borrow self mutably.
        if let Some(rx) = self.local.take() {
            loop {
                match rx.try_recv() {
                    Ok(ev) => self.on_agent(ev),
                    Err(TryRecvError::Empty) => {
                        self.local = Some(rx);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        if self.phase == Phase::Splash && now.saturating_sub(self.boot_at) >= SPLASH_MS {
            self.phase = Phase::Chat;
            self.chat_at = now;
        }
    }

    pub fn is_animating(&self) -> bool {
        if self.phase == Phase::Splash || self.busy {
            return true;
        }
        // The welcome screen keeps its own gentle motion.
        if self.is_empty_state() {
            return true;
        }
        if self.now.saturating_sub(self.chat_at) < 400
            || self.now.saturating_sub(self.status.cost_flash) < 700
            || self.now.saturating_sub(self.sent_at) < 600
            || self.now.saturating_sub(self.cancel_at) < 600
        {
            return true;
        }
        if !self.status.anim_in.done(self.now) || !self.status.anim_out.done(self.now) {
            return true;
        }
        if self.items.iter().any(|i| self.now.saturating_sub(i.born()) < 300) {
            return true;
        }
        // A card that just landed is still settling (or shaking).
        if self.items.iter().any(|i| match i {
            Item::Tool {
                state: ToolState::Done { at, .. },
                ..
            } => self.now.saturating_sub(*at) < 700,
            Item::Assistant { finished, .. } => {
                *finished > 0 && self.now.saturating_sub(*finished) < 700
            }
            Item::Reasoning { born, .. } => self.now.saturating_sub(*born) < 200,
            _ => false,
        }) {
            return true;
        }
        if let Some(p) = &self.popup {
            if self.now.saturating_sub(p.opened_at) < 260
                || self.now.saturating_sub(p.sel_at) < 200
            {
                return true;
            }
        }
        if self.help && self.now.saturating_sub(self.help_at) < 260 {
            return true;
        }
        false
    }

    pub fn cursor_on(&self) -> bool {
        (self.now / 530) % 2 == 0
    }

    // -- input handling ---------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if self.phase == Phase::Splash {
            self.skip_splash();
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.ctrl_c();
                    return;
                }
                KeyCode::Char('d') => {
                    self.quit = true;
                    return;
                }
                KeyCode::Char('j') => {
                    self.input.newline();
                    return;
                }
                KeyCode::Char('l') => {
                    self.clear_screen = true;
                    return;
                }
                KeyCode::Char('e') => {
                    self.toggle_last_tool();
                    return;
                }
                KeyCode::Char('o') => {
                    self.toggle_expand_all();
                    return;
                }
                KeyCode::Char('u') => {
                    self.input.clear();
                    self.popup = None;
                    return;
                }
                KeyCode::Char('w') => {
                    self.input.kill_word();
                    return;
                }
                _ => {}
            }
        }
        if self.help {
            self.help = false;
            return;
        }
        if key.code == KeyCode::Esc {
            self.popup = None;
            return;
        }

        if self.popup.is_some() {
            match key.code {
                KeyCode::Up => {
                    let now = self.now;
                    if let Some(p) = &mut self.popup {
                        p.select(p.sel.saturating_sub(1), now);
                    }
                    return;
                }
                KeyCode::Down => {
                    let now = self.now;
                    if let Some(p) = &mut self.popup {
                        let n = p.matches().len();
                        p.select((p.sel + 1).min(n.saturating_sub(1)), now);
                    }
                    return;
                }
                KeyCode::Tab | KeyCode::Enter => {
                    self.accept_popup();
                    return;
                }
                KeyCode::Backspace => {
                    self.input.backspace();
                    self.sync_popup();
                    return;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Enter => self.send(),
            KeyCode::Tab => {
                if self.popup.is_none() && self.input.text().starts_with('/') {
                    self.sync_popup();
                }
            }
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Up => {
                if self.input.is_empty() || !self.input.move_up() {
                    self.input.hist_prev();
                }
            }
            KeyCode::Down => {
                if !self.input.move_down() {
                    self.input.hist_next();
                }
            }
            KeyCode::PageUp => self.scroll_by(self.view_h.get() as isize - 2),
            KeyCode::PageDown => self.scroll_by(-(self.view_h.get() as isize - 2)),
            KeyCode::Char(c) => {
                self.input.insert_char(c);
                self.sync_popup();
            }
            _ => {}
        }
    }

    fn skip_splash(&mut self) {
        if self.phase == Phase::Splash {
            self.phase = Phase::Chat;
            self.chat_at = self.now;
        }
    }

    fn ctrl_c(&mut self) {
        if self.busy {
            self.cancel_run();
            return;
        }
        if self.now.saturating_sub(self.ctrl_c_at) < 2000 {
            self.quit = true;
        } else {
            self.ctrl_c_at = self.now;
            self.note("press ctrl+c again to quit · ctrl+d leaves now", false);
        }
    }

    pub fn cancel_run(&mut self) {
        if let Some(agent) = &self.agent {
            agent.cancel();
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let next = self.scroll as isize + delta;
        self.scroll = next.max(0) as usize;
    }

    fn toggle_last_tool(&mut self) {
        if let Some(Item::Tool { expanded, expand_at, .. }) = self
            .items
            .iter_mut()
            .rev()
            .find(|i| matches!(i, Item::Tool { .. }))
        {
            *expanded = !*expanded;
            *expand_at = self.now;
        }
    }

    /// Open every card in the transcript, or close them all.
    fn toggle_expand_all(&mut self) {
        let any_open = self.items.iter().any(|i| matches!(i, Item::Tool { expanded: true, .. }));
        let now = self.now;
        for item in self.items.iter_mut() {
            if let Item::Tool { expanded, expand_at, .. } = item {
                *expanded = !any_open;
                *expand_at = now;
            }
        }
    }

    fn sync_popup(&mut self) {
        let text = self.input.text();
        let is_cmd = text.starts_with('/') && !text.contains('\n') && !text[1..].contains(' ');
        if is_cmd {
            let filter = text[1..].to_string();
            let now = self.now;
            match &mut self.popup {
                Some(p) => {
                    if p.filter != filter {
                        p.filter = filter;
                        p.select(0, now);
                    }
                }
                None => self.popup = Some(Popup::new(filter, now)),
            }
        } else {
            self.popup = None;
        }
    }

    fn accept_popup(&mut self) {
        let Some(p) = &self.popup else { return };
        let matches = p.matches();
        let Some(cmd) = matches.get(p.sel).copied() else {
            self.popup = None;
            return;
        };
        self.popup = None;
        if cmd.args.is_empty() {
            // No arguments: run it like the user typed it.
            self.input.set_text(&format!("/{}", cmd.name));
            self.send();
        } else {
            self.input.set_text(&format!("/{} ", cmd.name));
        }
    }

    fn send(&mut self) {
        let text = self.input.text().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.input.push_history(&text);
        self.input.clear();
        self.popup = None;
        self.scroll = 0;
        if let Some(cmd) = text.strip_prefix('/') {
            self.run_command(cmd.trim());
            return;
        }
        let Some(agent) = &self.agent else {
            self.note("no engine attached in this mode", true);
            return;
        };
        self.sent_at = self.now;
        self.items.push(Item::User {
            text: text.clone(),
            born: self.now,
        });
        self.busy = true;
        agent.say(&text);
    }

    // -- commands ---------------------------------------------------------

    fn run_command(&mut self, cmd: &str) {
        let mut parts = cmd.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim().to_string();
        match name {
            "clear" => {
                self.items.clear();
                if let Some(agent) = &self.agent {
                    agent.new_session();
                }
                self.note("conversation cleared", false);
            }
            "new" => {
                self.items.clear();
                if let Some(agent) = &self.agent {
                    agent.new_session();
                }
                self.note("new session", false);
            }
            "help" => {
                self.help = true;
                self.help_at = self.now;
            }
            "cost" => {
                let s = &self.status;
                let cost = match s.cost {
                    Some(c) if c == 0.0 => "free".to_string(),
                    Some(c) => format!("${c:.4}"),
                    None => "unpriced".to_string(),
                };
                let line = format!(
                    "session: {cost} · {} in / {} out · context {}% of {}K",
                    crate::textutil::short_num(s.tokens_in),
                    crate::textutil::short_num(s.tokens_out),
                    s.ctx_pct(self.now),
                    s.ctx_max / 1000
                );
                self.note(&line, false);
            }
            "compact" => {
                if let Some(agent) = &self.agent {
                    agent.compact();
                    self.busy = true;
                    self.note("summarizing the older half of the conversation", false);
                }
            }
            "doctor" => self.spawn_tool("doctor", "{\"network\":\"skip\"}"),
            "skills" => self.spawn_tool("skills_list", "{}"),
            "init" => self.write_agents_md(),
            "sessions" => self.list_sessions(),
            "theme" => {
                let level = match self.theme.level {
                    crate::theme::ColorLevel::True => "truecolor",
                    crate::theme::ColorLevel::Indexed => "256 colors",
                    crate::theme::ColorLevel::Basic => "16 colors",
                };
                self.note(
                    &format!(
                        "detected {level}. force another with HARNESS_COLOR=true|256|16, and set \
                         HARNESS_BG=rrggbb if your terminal is light"
                    ),
                    false,
                );
            }
            "model" => self.switch_model(&rest),
            "provider" => self.switch_provider(&rest),
            "quit" | "q" => self.quit = true,
            "" => {}
            other => self.note(&format!("unknown command: /{other}"), true),
        }
    }

    fn switch_model(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            let model = self.status.model.clone();
            let provider = self.status.provider.clone();
            self.note(
                &format!("model: {model} · provider {provider} · switch with /model <name>"),
                false,
            );
            return;
        }
        let current = self.status.provider.clone();
        let Some(cfg) = self.config.as_mut() else {
            self.note("no config loaded", true);
            return;
        };
        let Some(provider) = cfg.find(&current).cloned() else {
            self.note("could not find the active provider in the config", true);
            return;
        };
        let mut updated = provider;
        if !updated.models.iter().any(|m| m == name) {
            updated.models.push(name.to_string());
        }
        let _ = cfg.set_active(&updated.name, name);
        if let Some(agent) = &mut self.agent {
            agent.swap(updated, name.to_string());
        }
        self.status.model = name.to_string();
        self.note(&format!("model set to {name}"), false);
    }

    fn switch_provider(&mut self, name: &str) {
        let name = name.trim();
        let models = self
            .config
            .as_ref()
            .map(|c| {
                c.providers
                    .iter()
                    .map(|p| format!("{} ({})", p.name, p.kind.as_str()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if name.is_empty() {
            let current = self.status.provider.clone();
            self.note(&format!("providers: {} · on {current}", models.join(", ")), false);
            return;
        }
        let Some(cfg) = self.config.as_mut() else {
            self.note("no config loaded", true);
            return;
        };
        let Some(provider) = cfg.find(name).cloned() else {
            self.note(&format!("unknown provider '{name}'. known: {}", models.join(", ")), true);
            return;
        };
        let model = provider
            .models
            .first()
            .cloned()
            .unwrap_or_else(|| cfg.model.clone());
        let _ = cfg.set_active(&provider.name, &model);
        let provider_name = provider.name.clone();
        if let Some(agent) = &mut self.agent {
            agent.swap(provider, model.clone());
        }
        self.status.provider = provider_name.clone();
        self.status.model = model.clone();
        self.note(&format!("using {provider_name} · {model}"), false);
    }

    fn write_agents_md(&mut self) {
        let root = std::env::current_dir().unwrap_or_else(|_| ".".into());
        let path = root.join("AGENTS.md");
        if path.exists() {
            self.note("AGENTS.md already exists, left alone", false);
            return;
        }
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "this project".into());
        let body = format!(
            "# {name}\n\nHow to work in here.\n\n## Build and test\n\n```sh\n# the commands that \
             matter\n```\n\n## Rules\n\n- \n"
        );
        match std::fs::write(&path, body) {
            Ok(_) => self.note("wrote AGENTS.md, fill in the blanks", false),
            Err(e) => self.note(&format!("could not write AGENTS.md: {e}"), true),
        }
    }

    fn list_sessions(&mut self) {
        let sessions = crate::session::Session::list();
        if sessions.is_empty() {
            self.note("no saved sessions yet", false);
            return;
        }
        let now = crate::session::unix_now();
        self.note(&format!("{} saved sessions, newest first:", sessions.len()), false);
        for (id, title, updated, _) in sessions.into_iter().take(8) {
            let age = now.saturating_sub(updated);
            let when = if age < 3600 {
                format!("{}m ago", age / 60)
            } else if age < 86_400 {
                format!("{}h ago", age / 3600)
            } else {
                format!("{}d ago", age / 86_400)
            };
            self.note(&format!("{id} · {when} · {title}"), false);
        }
        self.note("resume one with: harness --continue <id>", false);
    }

    /// Run a tool from the UI, showing it in the transcript like any other.
    fn spawn_tool(&mut self, name: &str, args: &'static str) {
        if self.local.is_some() {
            self.note("a local check is already running", false);
            return;
        }
        let root = std::env::current_dir().unwrap_or_else(|_| ".".into());
        let (tx, rx) = std::sync::mpsc::channel();
        let name = name.to_string();
        std::thread::spawn(move || {
            let mut ctx = crate::tools::Ctx::new(root, crate::llm::http::Cancel::new(), "ui".into());
            let _ = tx.send(AgentEvent::ToolStart { name: name.clone(), args: args.to_string() });
            let started = std::time::Instant::now();
            let result = crate::tools::run(&mut ctx, &name, args);
            let ms = started.elapsed().as_millis() as u64;
            let (ok, output) = match result {
                Ok(text) => (true, crate::tools::clip(text)),
                Err(err) => (false, err),
            };
            let _ = tx.send(AgentEvent::ToolDone { name, output, ok, ms });
            let _ = tx.send(AgentEvent::Done);
        });
        self.local = Some(rx);
    }

    pub fn note(&mut self, text: &str, error: bool) {
        self.items.push(Item::Note {
            text: text.to_string(),
            born: self.now,
            error,
        });
    }

    // -- agent events -----------------------------------------------------

    pub fn on_agent(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Thinking => {
                if !matches!(self.items.last(), Some(Item::Thinking { .. })) {
                    self.items.push(Item::Thinking { born: self.now });
                }
            }
            AgentEvent::Reasoning(text) => {
                match self.items.last_mut() {
                    Some(Item::Reasoning { text: body, .. }) => body.push_str(&text),
                    _ => self.items.push(Item::Reasoning { text, born: self.now }),
                }
            }
            AgentEvent::Token(t) => {
                if matches!(self.items.last(), Some(Item::Thinking { .. })) {
                    self.items.pop();
                }
                match self.items.last_mut() {
                    Some(Item::Assistant { text, streaming, .. }) if *streaming => text.push_str(&t),
                    _ => self.items.push(Item::Assistant {
                        text: t,
                        born: self.now,
                        streaming: true,
                        finished: 0,
                    }),
                }
            }
            AgentEvent::ToolStart { name, args } => {
                self.close_stream();
                self.items.push(Item::Tool {
                    name,
                    args,
                    state: ToolState::Running { started: self.now },
                    born: self.now,
                    expanded: false,
                    expand_at: 0,
                });
            }
            AgentEvent::ToolDone { name, output, ok, ms } => {
                if let Some(Item::Tool { name: n, state, .. }) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, Item::Tool { .. }))
                {
                    if *n == name {
                        *state = ToolState::Done {
                            ok,
                            ms,
                            output,
                            at: self.now,
                        };
                    }
                }
            }
            AgentEvent::Usage { tokens_in, tokens_out, cost } => {
                self.status.set_usage(tokens_in, tokens_out, cost, self.now);
            }
            AgentEvent::Notice(text) => self.note(&text, false),
            AgentEvent::Error(text) => {
                self.close_stream();
                self.busy = false;
                self.note(&text, true);
            }
            AgentEvent::Session(path) => {
                self.status.session = path;
            }
            AgentEvent::Cancelled => {
                self.close_stream();
                self.busy = false;
                self.cancel_at = self.now;
                self.note("cancelled", true);
            }
            AgentEvent::Done => {
                self.close_stream();
                self.busy = false;
            }
        }
    }

    fn close_stream(&mut self) {
        if matches!(self.items.last(), Some(Item::Thinking { .. })) {
            self.items.pop();
        }
        if let Some(Item::Assistant {
            streaming,
            finished,
            ..
        }) = self.items.iter_mut().rev().find(|i| matches!(i, Item::Assistant { .. }))
        {
            if *streaming {
                *streaming = false;
                *finished = self.now;
            }
        }
    }

    // -- drawing ----------------------------------------------------------

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 16 || area.height < 5 {
            return;
        }
        if self.phase == Phase::Splash {
            ui::splash::draw(frame, area, self);
            return;
        }
        ui::draw(frame, area, self);
    }

    /// Fade-in factor for things that just appeared.
    pub fn appear(&self, born: u64) -> f32 {
        Tween::new(240, anim::Ease::OutCubic).t(self.now, born)
    }
}
