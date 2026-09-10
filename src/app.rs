//! Application state and the event loop's brain.
//!
//! Everything time-dependent reads `self.now`, which main updates each frame
//! from the monotonic clock. Frame dumps set it by hand so animations can be
//! inspected mid-flight.

use crate::anim::{self, Tween};
use crate::demo::{AgentEvent, Demo};
use crate::input::Input;
use crate::theme::Theme;
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::cell::Cell;

pub const SPLASH_MS: u64 = 1500;

#[derive(PartialEq, Clone, Copy)]
pub enum Phase {
    Splash,
    Chat,
}

pub enum ToolState {
    Running { started: u64 },
    Done { ok: bool, ms: u64, output: String },
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
    Thinking {
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
            | Item::Tool { born, .. }
            | Item::Note { born, .. } => *born,
        }
    }
}

pub struct Status {
    pub model: String,
    pub workspace: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub ctx_max: u64,
    pub cost: f64,
    pub cost_flash: u64,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            model: "glm-5.3-flash".into(),
            workspace: std::env::current_dir()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| "~".into()),
            tokens_in: 0,
            tokens_out: 0,
            ctx_max: 128_000,
            cost: 0.0,
            cost_flash: 0,
        }
    }
}

pub struct Command {
    pub name: &'static str,
    pub args: &'static str,
    pub desc: &'static str,
}

pub const COMMANDS: &[Command] = &[
    Command { name: "clear", args: "", desc: "reset the conversation" },
    Command { name: "compact", args: "", desc: "summarize older messages" },
    Command { name: "cost", args: "", desc: "spend for this session" },
    Command { name: "doctor", args: "", desc: "self-test every tool" },
    Command { name: "help", args: "", desc: "keys and commands" },
    Command { name: "init", args: "", desc: "write AGENTS.md for this repo" },
    Command { name: "model", args: "<name>", desc: "switch the model" },
    Command { name: "plan", args: "", desc: "propose before acting" },
    Command { name: "skills", args: "", desc: "list installed skills" },
    Command { name: "theme", args: "dark|light", desc: "match your terminal" },
];

pub struct Popup {
    pub filter: String,
    pub sel: usize,
    pub opened_at: u64,
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
    pub demo: Demo,
    pub quit: bool,
    pub clear_screen: bool,
    pub ctrl_c_at: u64,
    pub view_h: Cell<u16>,
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
            demo: Demo::start(),
            quit: false,
            clear_screen: false,
            ctrl_c_at: 0,
            view_h: Cell::new(20),
        }
    }

    pub fn update(&mut self, now: u64) {
        self.now = now;
        for ev in self.demo.poll() {
            self.on_agent(ev);
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
        if self.now.saturating_sub(self.chat_at) < 400
            || self.now.saturating_sub(self.status.cost_flash) < 700
        {
            return true;
        }
        if self.items.iter().any(|i| self.now.saturating_sub(i.born()) < 300) {
            return true;
        }
        if let Some(p) = &self.popup {
            if self.now.saturating_sub(p.opened_at) < 260 {
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
                    if let Some(p) = &mut self.popup {
                        p.sel = p.sel.saturating_sub(1);
                    }
                    return;
                }
                KeyCode::Down => {
                    if let Some(p) = &mut self.popup {
                        let n = p.matches().len();
                        p.sel = (p.sel + 1).min(n.saturating_sub(1));
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
        self.phase = Phase::Chat;
        self.chat_at = self.now;
    }

    fn ctrl_c(&mut self) {
        if self.busy {
            self.demo.cancel();
            return;
        }
        if self.now.saturating_sub(self.ctrl_c_at) < 2000 {
            self.quit = true;
        } else {
            self.ctrl_c_at = self.now;
            self.note("press ctrl+c again to quit", false);
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

    fn sync_popup(&mut self) {
        let text = self.input.text();
        let is_cmd = text.starts_with('/') && !text.contains('\n') && !text[1..].contains(' ');
        if is_cmd {
            let filter = text[1..].to_string();
            match &mut self.popup {
                Some(p) => {
                    if p.filter != filter {
                        p.filter = filter;
                        p.sel = 0;
                    }
                }
                None => {
                    self.popup = Some(Popup {
                        filter,
                        sel: 0,
                        opened_at: self.now,
                    });
                }
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
        self.items.push(Item::User {
            text: text.clone(),
            born: self.now,
        });
        self.busy = true;
        self.demo.send(&text);
    }

    fn run_command(&mut self, cmd: &str) {
        let name = cmd.split_whitespace().next().unwrap_or("");
        match name {
            "clear" => {
                self.items.clear();
                self.note("conversation cleared", false);
            }
            "help" => {
                self.help = true;
                self.help_at = self.now;
            }
            "cost" => {
                let s = &self.status;
                self.note(
                    &format!(
                        "this session: ${:.4} · {} in / {} out · {}K context",
                        s.cost,
                        crate::textutil::short_num(s.tokens_in),
                        crate::textutil::short_num(s.tokens_out),
                        s.ctx_max / 1000
                    ),
                    false,
                );
            }
            "compact" => {
                self.note("older messages summarized, context at 18%", false);
            }
            "doctor" => {
                self.note("running doctor", false);
                self.busy = true;
                self.demo.send("__doctor");
            }
            "skills" => {
                self.note("3 skills installed: commit-style, rust-review, termux-notes", false);
            }
            "init" | "plan" | "model" | "theme" => {
                self.note(&format!("/{name} lands with the engine, UI only for now"), false);
            }
            "" => {}
            other => {
                self.note(&format!("unknown command: /{other}"), true);
            }
        }
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
                        *state = ToolState::Done { ok, ms, output };
                    }
                }
            }
            AgentEvent::Usage { tokens_in, tokens_out, cost } => {
                self.status.tokens_in = tokens_in;
                self.status.tokens_out = tokens_out;
                self.status.cost = cost;
                self.status.cost_flash = self.now;
            }
            AgentEvent::Cancelled => {
                self.close_stream();
                self.busy = false;
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
