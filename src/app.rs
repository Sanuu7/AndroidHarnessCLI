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

/// "12m ago", for session lists.
fn age(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Second line of a model row: the one thing worth knowing about it here.
fn model_hint(id: &str, provider: &str) -> String {
    if crate::llm::models::free_slot(id) {
        "free".to_string()
    } else {
        provider.to_string()
    }
}

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
        /// When the answer started, so the block can collapse to one line.
        finished: u64,
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
    pub fn born(&self) -> u64 {        match self {
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
    Command { name: "sessions", args: "[id]", desc: "resume a past session" },
    Command { name: "skills", args: "", desc: "list installed skills" },
    Command { name: "theme", args: "", desc: "match your terminal" },
];

/// Commands that open a list instead of acting right away.
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Pick {
    Model,
    Provider,
    Session,
}

impl Pick {
    pub fn of(command: &str) -> Option<Pick> {
        match command {
            "model" => Some(Pick::Model),
            "provider" => Some(Pick::Provider),
            "sessions" | "resume" => Some(Pick::Session),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Pick::Model => "model",
            Pick::Provider => "provider",
            Pick::Session => "session",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Pick::Model => "enter to switch, type to filter or add",
            Pick::Provider => "enter to switch",
            Pick::Session => "enter to resume",
        }
    }
}

/// One row in a picker.
#[derive(Clone)]
pub struct Choice {
    pub label: String,
    pub hint: String,
    /// The value that gets applied, which can differ from the label.
    pub value: String,
    pub current: bool,
}

impl Choice {
    pub fn new(label: impl Into<String>, hint: impl Into<String>) -> Self {
        let label = label.into();
        Self { value: label.clone(), label, hint: hint.into(), current: false }
    }

    pub fn current(mut self, yes: bool) -> Self {
        self.current = yes;
        self
    }
}

/// The popup above the composer: the command palette, or a list to pick from.
pub struct Popup {
    /// None while the command palette is up.
    pub pick: Option<Pick>,
    /// Text typed after the command name, used to filter.
    pub filter: String,
    pub choices: Vec<Choice>,
    /// A live list is on the way; the footer says so.
    pub loading: bool,
    pub sel: usize,
    pub opened_at: u64,
    /// Where the highlight was before, so it can cross-fade between rows.
    pub sel_prev: usize,
    pub sel_at: u64,
}

impl Popup {
    pub fn commands(filter: String, now: u64) -> Self {
        Self {
            pick: None,
            filter,
            choices: Vec::new(),
            loading: false,
            sel: 0,
            opened_at: now,
            sel_prev: 0,
            sel_at: now,
        }
    }

    pub fn picker(pick: Pick, filter: String, choices: Vec<Choice>, now: u64) -> Self {
        let sel = choices.iter().position(|c| c.current).unwrap_or(0);
        Self {
            pick: Some(pick),
            filter,
            choices,
            loading: false,
            sel,
            opened_at: now,
            sel_prev: sel,
            sel_at: now,
        }
    }

    pub fn commands_matching(&self) -> Vec<&'static Command> {
        let f = self.filter.to_lowercase();
        COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(&f) || c.name.contains(&f))
            .collect()
    }

    /// Rows the user can see right now: the filtered choices.
    pub fn visible(&self) -> Vec<&Choice> {
        let f = self.filter.trim().to_lowercase();
        self.choices
            .iter()
            .filter(|c| f.is_empty() || c.label.to_lowercase().contains(&f))
            .collect()
    }

    pub fn rows(&self) -> usize {
        match self.pick {
            Some(_) => self.visible().len(),
            None => self.commands_matching().len(),
        }
    }

    /// The highlighted row, if the list has one.
    pub fn selected(&self) -> Option<Choice> {
        self.visible().get(self.sel).map(|c| (*c).clone())
    }

    /// Move the highlight, remembering where it came from.
    pub fn select(&mut self, sel: usize, now: u64) {
        if sel != self.sel {
            self.sel_prev = self.sel;
            self.sel = sel;
            self.sel_at = now;
        }
    }

    /// Replace the choice list, keeping the highlight on something sensible.
    pub fn set_choices(&mut self, choices: Vec<Choice>, now: u64) {
        let was = self.selected().map(|c| c.value);
        self.choices = choices;
        self.loading = false;
        let sel = match &was {
            Some(value) => self
                .visible()
                .iter()
                .position(|c| &c.value == value)
                .unwrap_or_else(|| self.visible().iter().position(|c| c.current).unwrap_or(0)),
            None => self.visible().iter().position(|c| c.current).unwrap_or(0),
        };
        self.select(sel, now);
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
    /// Reasoning blocks and tool cards stay open after ctrl+o.
    pub expand: bool,
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
            expand: false,
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
                KeyCode::Up | KeyCode::BackTab => {
                    let now = self.now;
                    if let Some(p) = &mut self.popup {
                        let rows = p.rows();
                        p.select(p.sel.saturating_sub(1).min(rows.saturating_sub(1)), now);
                    }
                    return;
                }
                KeyCode::Down => {
                    let now = self.now;
                    if let Some(p) = &mut self.popup {
                        let rows = p.rows();
                        p.select((p.sel + 1).min(rows.saturating_sub(1)), now);
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
        self.expand = !any_open;
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
        if !text.starts_with('/') || text.contains('\n') {
            self.popup = None;
            return;
        }
        let now = self.now;
        let body = &text[1..];
        // "/model lin" is a picker with a filter. "/model" on its own is still
        // the command palette, so the name can be finished with tab.
        if let Some((name, filter)) = body.split_once(' ') {
            if let Some(pick) = Pick::of(name) {
                let filter = filter.to_string();
                let same = matches!(&self.popup, Some(p) if p.pick == Some(pick));
                if same {
                    if let Some(p) = &mut self.popup {
                        if p.filter != filter {
                            p.filter = filter;
                            let rows = p.rows();
                            p.select(0.min(rows.saturating_sub(1)), now);
                        }
                    }
                    return;
                }
                let choices = self.choices_for(pick);
                let empty = choices.is_empty();
                self.popup = Some(Popup::picker(pick, filter, choices, now));
                if empty || pick == Pick::Model {
                    self.load_models();
                }
                return;
            }
        }
        if !body.contains(' ') {
            match &mut self.popup {
                Some(p) if p.pick.is_none() => {
                    if p.filter != body {
                        p.filter = body.to_string();
                        p.select(0, now);
                    }
                }
                _ => self.popup = Some(Popup::commands(body.to_string(), now)),
            }
        } else {
            self.popup = None;
        }
    }

    /// Rows for a picker, from whatever the config already knows.
    fn choices_for(&self, pick: Pick) -> Vec<Choice> {
        match pick {
            Pick::Model => {
                let current = self.status.model.clone();
                let provider = self.status.provider.clone();
                let mut ids: Vec<String> = self
                    .config
                    .as_ref()
                    .and_then(|c| c.find(&provider).cloned())
                    .map(|p| p.models)
                    .unwrap_or_default();
                if !ids.iter().any(|m| m == &current) {
                    ids.insert(0, current.clone());
                }
                ids.sort();
                ids.dedup();
                ids.into_iter()
                    .map(|id| {
                        Choice::new(id.clone(), model_hint(&id, &provider)).current(id == current)
                    })
                    .collect()
            }
            Pick::Provider => self
                .config
                .as_ref()
                .map(|c| {
                    c.providers
                        .iter()
                        .map(|p| {
                            Choice::new(p.name.clone(), p.kind.as_str())
                                .current(p.name == self.status.provider)
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Pick::Session => {
                let now = crate::session::unix_now();
                crate::session::Session::list()
                    .into_iter()
                    .take(30)
                    .map(|(_id, title, updated, path)| {
                        let mut choice = Choice::new(title, age(now, updated));
                        choice.value = path.display().to_string();
                        choice
                    })
                    .collect()
            }
        }
    }

    /// Ask the provider what it serves, and fill the picker when it answers.
    fn load_models(&mut self) {
        if self.local.is_some() {
            return;
        }
        let provider = self
            .config
            .as_ref()
            .and_then(|c| c.find(&self.status.provider).cloned());
        let Some(provider) = provider else { return };
        if let Some(popup) = self.popup.as_mut() {
            popup.loading = true;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let models = crate::llm::models::list(&provider).unwrap_or_default();
            let _ = tx.send(AgentEvent::Models(models));
        });
        self.local = Some(rx);
    }

    fn accept_popup(&mut self) {
        let Some(popup) = &self.popup else { return };
        let pick = popup.pick;
        let selected = popup.selected().map(|c| c.value);
        let typed = popup.filter.trim().to_string();
        let Some(pick) = pick else {
            let matches = popup.commands_matching();
            let Some(cmd) = matches.get(popup.sel).copied() else {
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
            return;
        };
        self.popup = None;
        self.input.clear();
        match pick {
            Pick::Model => {
                // Nothing matched but something was typed: take it as a model
                // name, which is how a new one gets added.
                let name = selected.clone().unwrap_or_else(|| typed.clone());
                if name.trim().is_empty() {
                    return;
                }
                self.apply_model(&name);
            }
            Pick::Provider => {
                if let Some(name) = selected {
                    self.apply_provider(&name);
                }
            }
            Pick::Session => {
                if let Some(path) = selected {
                    self.resume_session(&path);
                }
            }
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
            "sessions" => {
                if rest.is_empty() {
                    self.open_picker(Pick::Session);
                } else {
                    self.resume_named(&rest);
                }
            }
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
            "model" => {
                if rest.is_empty() {
                    self.open_picker(Pick::Model);
                } else {
                    self.apply_model(&rest);
                }
            }
            "provider" => {
                if rest.is_empty() {
                    self.open_picker(Pick::Provider);
                } else {
                    self.apply_provider(&rest);
                }
            }
            "quit" | "q" => self.quit = true,
            "" => {}
            other => self.note(&format!("unknown command: /{other}"), true),
        }
    }

    /// Put the command into the composer and let the picker logic open.
    fn open_picker(&mut self, pick: Pick) {
        self.input.set_text(&format!("/{} ", pick.title()));
        self.sync_popup();
    }

    fn apply_model(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
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

    fn apply_provider(&mut self, name: &str) {
        let name = name.trim();
        let known = self
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
            self.note(&format!("providers: {} · on {current}", known.join(", ")), false);
            return;
        }
        let Some(cfg) = self.config.as_mut() else {
            self.note("no config loaded", true);
            return;
        };
        let Some(provider) = cfg.find(name).cloned() else {
            self.note(&format!("unknown provider '{name}'. known: {}", known.join(", ")), true);
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

    /// Resume the newest session whose id starts with this, or whose title
    /// contains it, since typing a long id on a phone is nobody's idea of fun.
    fn resume_named(&mut self, want: &str) {
        let dir = crate::session::Session::dir();
        let exact = dir.join(format!("{want}.json"));
        let path = if exact.exists() {
            Some(exact)
        } else {
            crate::session::Session::list()
                .into_iter()
                .find(|(id, title, _, _)| {
                    id.starts_with(want) || title.to_lowercase().contains(&want.to_lowercase())
                })
                .map(|(_, _, _, path)| path)
        };
        match path {
            Some(path) => self.resume_session(&path.display().to_string()),
            None => self.note(&format!("no session matching '{want}'"), true),
        }
    }

    fn resume_session(&mut self, path: &str) {
        let Some(session) = crate::session::Session::load(&std::path::PathBuf::from(path)) else {
            self.note("that session could not be read", true);
            return;
        };
        let Some(agent) = &self.agent else {
            self.note("no engine attached", true);
            return;
        };
        self.items.clear();
        for msg in &session.messages {
            match msg.role {
                crate::llm::Role::User => self.items.push(Item::User {
                    text: msg.text.clone(),
                    born: self.now,
                }),
                crate::llm::Role::Assistant => self.items.push(Item::Assistant {
                    text: msg.text.clone(),
                    born: self.now,
                    streaming: false,
                    finished: 0,
                }),
                crate::llm::Role::Tool => self.items.push(Item::Tool {
                    name: msg.name.clone(),
                    args: String::new(),
                    state: ToolState::Done {
                        ok: !msg.error,
                        ms: 0,
                        output: msg.text.clone(),
                        at: self.now,
                    },
                    born: self.now,
                    expanded: false,
                    expand_at: 0,
                }),
            }
        }
        let title = session.title.clone();
        let messages = session.messages.len();
        let model = session.model.clone();
        agent.load(session);
        self.note(&format!("resumed {title} ({messages} messages)"), false);
        if !model.is_empty() && model != self.status.model {
            self.note(&format!("that session used {model}"), false);
        }
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
                self.clear_thinking();
                self.items.push(Item::Thinking { born: self.now });
            }
            AgentEvent::Reasoning(text) => {
                // The placeholder has done its job the moment thinking starts
                // arriving, and it must never outlive the answer.
                self.clear_thinking();
                match self.items.last_mut() {
                    Some(Item::Reasoning { text: body, .. }) => body.push_str(&text),
                    _ => self.items.push(Item::Reasoning {
                        text,
                        born: self.now,
                        finished: 0,
                    }),
                }
            }
            AgentEvent::Token(t) => {
                self.clear_thinking();
                self.settle_reasoning();
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
                self.clear_thinking();
                self.settle_reasoning();
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
            AgentEvent::Models(models) => self.fill_models(models),
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

    /// A live model list arrived: fold it into the open picker.
    fn fill_models(&mut self, models: Vec<String>) {
        if models.is_empty() {
            return;
        }
        let current = self.status.model.clone();
        let provider = self.status.provider.clone();
        let mut choices = vec![Choice::new(current.clone(), model_hint(&current, &provider))
            .current(true)];
        for id in models {
            if id != current {
                let hint = model_hint(&id, &provider);
                choices.push(Choice::new(id, hint));
            }
        }
        let now = self.now;
        if let Some(popup) = self.popup.as_mut() {
            if popup.pick == Some(Pick::Model) {
                popup.set_choices(choices, now);
            }
        }
    }

    /// Drop the "waiting for the first token" placeholder. It is only ever
    /// true before anything has arrived, so anything real removes it.
    fn clear_thinking(&mut self) {
        self.items.retain(|i| !matches!(i, Item::Thinking { .. }));
    }

    /// The reasoning block is over once the answer or a tool call starts.
    fn settle_reasoning(&mut self) {
        if let Some(Item::Reasoning { finished, .. }) = self
            .items
            .iter_mut()
            .rev()
            .find(|i| matches!(i, Item::Reasoning { .. }))
        {
            if *finished == 0 {
                *finished = self.now;
            }
        }
    }

    fn close_stream(&mut self) {
        self.clear_thinking();
        self.settle_reasoning();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorLevel;

    fn app() -> App {
        let mut app = App::new(Theme::forced(ColorLevel::True));
        app.now = 10_000;
        app
    }

    fn has_thinking(app: &App) -> bool {
        app.items.iter().any(|i| matches!(i, Item::Thinking { .. }))
    }

    #[test]
    fn the_thinking_placeholder_never_outlives_the_answer() {
        let mut app = app();
        app.on_agent(AgentEvent::Thinking);
        assert!(has_thinking(&app));
        app.on_agent(AgentEvent::Token("here".into()));
        assert!(!has_thinking(&app), "the spinner must not sit above the answer");
        assert!(matches!(app.items.last(), Some(Item::Assistant { .. })));
    }

    #[test]
    fn reasoning_also_clears_the_placeholder() {
        let mut app = app();
        app.on_agent(AgentEvent::Thinking);
        app.on_agent(AgentEvent::Reasoning("hmm".into()));
        assert!(!has_thinking(&app));
        assert!(matches!(app.items.last(), Some(Item::Reasoning { .. })));
        // A second chunk appends instead of opening a new block.
        app.on_agent(AgentEvent::Reasoning(" more".into()));
        assert_eq!(app.items.len(), 1);
        match &app.items[0] {
            Item::Reasoning { text, .. } => assert_eq!(text, "hmm more"),
            _ => panic!("expected reasoning"),
        }
    }

    #[test]
    fn reasoning_collapses_when_the_answer_starts() {
        let mut app = app();
        app.on_agent(AgentEvent::Reasoning("thinking about it".into()));
        app.now = 12_500;
        app.on_agent(AgentEvent::Token("answer".into()));
        match &app.items[0] {
            Item::Reasoning { finished, .. } => {
                assert_eq!(*finished, 12_500, "the block knows when it ended")
            }
            _ => panic!("expected reasoning"),
        }
    }

    #[test]
    fn a_tool_call_ends_the_reasoning_too() {
        let mut app = app();
        app.on_agent(AgentEvent::Reasoning("let me look".into()));
        app.on_agent(AgentEvent::ToolStart { name: "grep".into(), args: "{}".into() });
        match &app.items[0] {
            Item::Reasoning { finished, .. } => assert!(*finished > 0),
            _ => panic!("expected reasoning"),
        }
        assert!(matches!(app.items.last(), Some(Item::Tool { .. })));
    }

    #[test]
    fn the_picker_filters_and_selects() {
        let choices = vec![
            Choice::new("gpt-5", "openai"),
            Choice::new("gpt-5-mini", "openai").current(true),
            Choice::new("o3", "openai"),
        ];
        let mut popup = Popup::picker(Pick::Model, String::new(), choices, 0);
        assert_eq!(popup.rows(), 3);
        assert_eq!(popup.sel, 1, "opens on the current model");
        assert_eq!(popup.selected().unwrap().label, "gpt-5-mini");
        popup.filter = "o3".into();
        assert_eq!(popup.rows(), 1);
        popup.select(0, 5);
        assert_eq!(popup.selected().unwrap().label, "o3");
    }

    #[test]
    fn a_typed_name_that_matches_nothing_is_not_a_selection() {
        let choices = vec![Choice::new("gpt-5", "openai")];
        let popup = Popup::picker(Pick::Model, "my-own-model".into(), choices, 0);
        assert!(popup.selected().is_none());
        assert_eq!(popup.filter, "my-own-model");
    }

    #[test]
    fn live_models_keep_the_highlight_on_the_same_row() {
        let choices = vec![
            Choice::new("a", "p").current(true),
            Choice::new("b", "p"),
        ];
        let mut popup = Popup::picker(Pick::Model, String::new(), choices, 0);
        popup.select(1, 10);
        let mut fresh = vec![Choice::new("a", "p").current(true)];
        fresh.push(Choice::new("b", "p"));
        fresh.push(Choice::new("c", "p"));
        popup.set_choices(fresh, 20);
        assert_eq!(popup.selected().unwrap().label, "b", "the row under the cursor stays");
        assert!(!popup.loading);
    }

    #[test]
    fn command_palette_still_filters_commands() {
        let popup = Popup::commands("mo".into(), 0);
        let names: Vec<&str> = popup.commands_matching().iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["model"]);
        assert_eq!(popup.rows(), 1);
    }

    #[test]
    fn pick_titles_open_the_right_list() {
        assert_eq!(Pick::of("model"), Some(Pick::Model));
        assert_eq!(Pick::of("provider"), Some(Pick::Provider));
        assert_eq!(Pick::of("sessions"), Some(Pick::Session));
        assert_eq!(Pick::of("cost"), None);
    }
}
