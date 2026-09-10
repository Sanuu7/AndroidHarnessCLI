//! The agent loop.
//!
//! One worker thread owns the conversation: it talks to the provider, runs
//! tools, compacts when the context fills up, and saves the session. The UI
//! thread only sends commands and drains events, so drawing never waits on
//! the network.

pub mod cost;
pub mod prompt;

use crate::config::Provider;
use crate::llm::{self, Delta, Msg, Role, ToolCall, Usage, Wire, http};
use crate::session::Session;
use crate::tools;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

/// Tool rounds before the agent stops on its own. Long enough for real work,
/// short enough that a loop cannot run all night.
pub const MAX_STEPS: usize = 24;
/// Share of the context window that triggers a compaction.
const COMPACT_AT: f64 = 0.72;
const COMPACT_PREFIX: &str = "[Auto-compacted context: summary of the earlier conversation]";

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A turn started; nothing has arrived from the model yet.
    Thinking,
    Reasoning(String),
    Token(String),
    ToolStart { name: String, args: String },
    ToolDone { name: String, output: String, ok: bool, ms: u64 },
    Usage { tokens_in: u64, tokens_out: u64, cost: Option<f64> },
    /// Something worth a line of its own: retries, compaction, limits.
    Notice(String),
    Error(String),
    Session(String),
    Done,
    Cancelled,
}

enum Cmd {
    Say(String),
    Cancel,
    New,
    Compact,
    Swap { provider: Box<Provider>, model: String },
    Load(Box<Session>),
    Shutdown,
}

/// The UI's handle on the worker.
pub struct Agent {
    tx: Sender<Cmd>,
    events: Receiver<AgentEvent>,
    cancel: http::Cancel,
    worker: Option<thread::JoinHandle<()>>,
    pub model: String,
    pub provider: String,
    pub ctx_max: u64,
}

impl Agent {
    pub fn start(provider: Provider, model: String, root: PathBuf, ctx_max: u64, prefs: Vec<String>) -> Agent {
        let (tx, rx) = channel::<Cmd>();
        let (etx, erx) = channel::<AgentEvent>();
        let cancel = http::Cancel::new();
        let worker_cancel = cancel.clone();
        let model0 = model.clone();
        let provider0 = provider.name.clone();
        // The worker and the handle each keep a copy of the labels.
        let (model_in, provider_in) = (model0.clone(), provider0.clone());
        let worker = thread::spawn(move || {
            let mut worker = Worker {
                provider,
                model,
                root,
                messages: Vec::new(),
                session: Session::new(&model_in, &provider_in),                tx: etx,
                cancel: worker_cancel,
                session_in: 0,
                session_out: 0,
                cost: None,
                ctx_max,
                prefs,
            };
            worker.run(rx);
        });
        Agent {
            tx,
            events: erx,
            cancel,
            worker: Some(worker),
            model: model0,
            provider: provider0,
            ctx_max,
        }
    }

    pub fn say(&self, text: &str) {
        let _ = self.tx.send(Cmd::Say(text.to_string()));
    }

    /// Stop the running turn. Cancelling the http handle kills curl, which
    /// makes the worker's read loop end immediately.
    pub fn cancel(&self) {
        self.cancel.cancel();
        let _ = self.tx.send(Cmd::Cancel);
    }

    pub fn new_session(&self) {
        let _ = self.tx.send(Cmd::New);
    }

    pub fn compact(&self) {
        let _ = self.tx.send(Cmd::Compact);
    }

    pub fn load(&self, session: Session) {
        let _ = self.tx.send(Cmd::Load(Box::new(session)));
    }

    pub fn swap(&mut self, provider: Provider, model: String) {
        self.provider = provider.name.clone();
        self.model = model.clone();
        let _ = self.tx.send(Cmd::Swap { provider: Box::new(provider), model });
    }

    pub fn poll(&self) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(ev) => out.push(ev),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.cancel.cancel();
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Worker {
    provider: Provider,
    model: String,
    root: PathBuf,
    messages: Vec<Msg>,
    session: Session,
    tx: Sender<AgentEvent>,
    cancel: http::Cancel,
    /// Prompt tokens of the last request: how full the context is.
    session_in: u64,
    session_out: u64,
    cost: Option<f64>,
    ctx_max: u64,
    prefs: Vec<String>,
}

fn emit(tx: &Sender<AgentEvent>, event: AgentEvent) {
    let _ = tx.send(event);
}

impl Worker {
    fn run(&mut self, rx: Receiver<Cmd>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Say(text) => {
                    self.messages.push(Msg::user(text));
                    self.turn();
                }
                Cmd::Cancel => {}
                Cmd::Compact => {
                    if self.compact_now() {
                        emit(&self.tx, AgentEvent::Notice("context compacted".into()));
                    } else {
                        emit(&self.tx, AgentEvent::Notice("nothing to compact".into()));
                    }
                    emit(&self.tx, AgentEvent::Done);
                }
                Cmd::New => {
                    self.messages.clear();
                    self.session = Session::new(&self.model, &self.provider.name);
                    self.session_in = 0;
                    self.session_out = 0;
                    self.cost = None;
                    emit(&self.tx, AgentEvent::Done);
                }
                Cmd::Swap { provider, model } => {
                    self.provider = *provider;
                    self.model = model;
                    self.session.model = self.model.clone();
                    self.session.provider = self.provider.name.clone();
                    emit(&self.tx, AgentEvent::Done);
                }
                Cmd::Load(session) => {
                    self.messages = session.messages.clone();
                    self.session = *session;
                    emit(&self.tx, AgentEvent::Done);
                }
                Cmd::Shutdown => break,
            }
        }
    }

    // -- one user turn ----------------------------------------------------

    fn turn(&mut self) {
        self.cancel.reset();
        let mut step = 0usize;
        while step < MAX_STEPS {
            step += 1;
            if self.cancel.cancelled() {
                emit(&self.tx, AgentEvent::Cancelled);
                self.finish();
                return;
            }
            self.maybe_compact();
            emit(&self.tx, AgentEvent::Thinking);
            let result = match self.stream_turn() {
                Ok(result) => result,
                Err(err) => {
                    if self.cancel.cancelled() {
                        emit(&self.tx, AgentEvent::Cancelled);
                        self.finish();
                        return;
                    }
                    if err.retryable {
                        emit(&self.tx, AgentEvent::Notice(format!("{} · retrying", err.message)));
                        match self.stream_turn() {
                            Ok(result) => result,
                            Err(second) => {
                                emit(&self.tx, AgentEvent::Error(second.message));
                                self.finish();
                                return;
                            }
                        }
                    } else {
                        emit(&self.tx, AgentEvent::Error(err.message));
                        self.finish();
                        return;
                    }
                }
            };

            self.account(&result.usage);
            let has_text = !result.text.trim().is_empty();
            self.messages.push(Msg::assistant(result.text, result.calls.clone()));
            if result.calls.is_empty() {
                break;
            }
            if !has_text {
                // Tool-only turns: nothing to say, the cards speak for themselves.
            }
            if !self.run_calls(&result.calls) {
                emit(&self.tx, AgentEvent::Cancelled);
                self.finish();
                return;
            }
            if step == MAX_STEPS {
                emit(
                    &self.tx,
                    AgentEvent::Notice(format!("stopped after {MAX_STEPS} tool rounds")),
                );
            }
        }
        self.finish();
    }

    fn finish(&mut self) {
        if let Err(e) = self.session_save() {
            emit(&self.tx, AgentEvent::Notice(format!("session not saved: {e}")));
        }
        emit(&self.tx, AgentEvent::Done);
    }

    fn session_save(&mut self) -> std::io::Result<()> {
        if self.messages.is_empty() {
            return Ok(());
        }
        self.session.messages = self.messages.clone();
        let path = self.session.save()?;
        emit(&self.tx, AgentEvent::Session(path.display().to_string()));
        Ok(())
    }

    /// Run the calls from one assistant turn. Read-only neighbours in the
    /// same batch go out together: exploring is what an agent does most, and
    /// a phone is happier with four greps in flight than four in a row.
    fn run_calls(&mut self, calls: &[ToolCall]) -> bool {
        let mut idx = 0usize;
        while idx < calls.len() {
            if self.cancel.cancelled() {
                return false;
            }
            let mut batch: Vec<&ToolCall> = Vec::new();
            let parallel_ok = tools::find(&calls[idx].name).map(|t| t.read_only).unwrap_or(false);
            if parallel_ok {
                while idx < calls.len() && batch.len() < 4 {
                    match tools::find(&calls[idx].name) {
                        Some(tool) if tool.read_only => {
                            batch.push(&calls[idx]);
                            idx += 1;
                        }
                        _ => break,
                    }
                }
            } else {
                batch.push(&calls[idx]);
                idx += 1;
            }
            let results = if batch.len() > 1 {
                self.run_parallel(&batch)
            } else {
                vec![self.run_one(batch[0])]
            };
            for (call, (ok, output)) in batch.iter().zip(results) {
                self.messages.push(Msg::tool(call, output, !ok));
            }
        }
        true
    }

    fn run_one(&self, call: &ToolCall) -> (bool, String) {
        let mut ctx = tools::Ctx::new(
            self.root.clone(),
            self.cancel.clone(),
            self.session.id.clone(),
        );
        emit(
            &self.tx,
            AgentEvent::ToolStart { name: call.name.clone(), args: call.args.clone() },
        );
        let started = Instant::now();
        let result = tools::run(&mut ctx, &call.name, &call.args);
        let ms = started.elapsed().as_millis() as u64;
        let (ok, output) = match result {
            Ok(text) => (true, tools::clip(text)),
            Err(err) => (false, err),
        };
        emit(
            &self.tx,
            AgentEvent::ToolDone { name: call.name.clone(), output: output.clone(), ok, ms },
        );
        (ok, output)
    }

    fn run_parallel(&self, batch: &[&ToolCall]) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|call| {
                    let (tx, root, cancel, session) = (
                        self.tx.clone(),
                        self.root.clone(),
                        self.cancel.clone(),
                        self.session.id.clone(),
                    );
                    scope.spawn(move || {
                        let mut ctx = tools::Ctx::new(root, cancel, session);
                        emit(
                            &tx,
                            AgentEvent::ToolStart { name: call.name.clone(), args: call.args.clone() },
                        );
                        let started = Instant::now();
                        let result = tools::run(&mut ctx, &call.name, &call.args);
                        let ms = started.elapsed().as_millis() as u64;
                        let (ok, output) = match result {
                            Ok(text) => (true, tools::clip(text)),
                            Err(err) => (false, err),
                        };
                        emit(
                            &tx,
                            AgentEvent::ToolDone {
                                name: call.name.clone(),
                                output: output.clone(),
                                ok,
                                ms,
                            },
                        );
                        (ok, output)
                    })
                })
                .collect();
            for handle in handles {
                out.push(handle.join().unwrap_or_else(|_| (false, "tool thread panicked".into())));
            }
        });
        out
    }

    // -- streaming --------------------------------------------------------

    fn stream_turn(&mut self) -> Result<TurnResult, StreamError> {
        let system = prompt::build(&self.root, &self.prefs);
        let schemas = tools::schemas();
        let turn = llm::Turn {
            model: &self.model,
            system: &system,
            messages: &self.messages,
            tools: &schemas,
            max_tokens: 8_192,
        };
        let (kind, provider) = llm::relay::route(&self.provider, &self.model);
        let mut wire: Box<dyn Wire> = llm::wire_for(kind);
        let mut request = wire.build(&provider, &turn);
        if self.provider.kind == crate::config::Kind::Relay {
            // The relay wants its session header whatever wire it answers on.
            request.headers.extend(llm::relay::extra_headers());
        }
        if std::env::var("HARNESS_DEBUG").is_ok() {
            // The body and headers go to disk so a rejected request can be
            // replayed with curl, unchanged.
            let dir = crate::config::Config::data_dir();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("last-request.json"), &request.body);
            let _ = std::fs::write(
                dir.join("last-request.headers"),
                request
                    .headers
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}\n"))
                    .collect::<String>(),
            );
        }
        let rx = http::post(request, self.cancel.clone()).map_err(|e| StreamError {
            message: e,
            retryable: false,
        })?;

        let mut result = TurnResult::default();
        let mut delta = Delta::default();
        loop {
            if self.cancel.cancelled() {
                return Err(StreamError { message: "cancelled".into(), retryable: false });
            }
            let msg = match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(msg) => msg,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            match msg {
                http::HttpMsg::Line(line) => {
                    let Some(payload) = sse_payload(&line) else { continue };
                    if payload.trim().is_empty() {
                        continue;
                    }
                    if let Err(err) = wire.parse(&payload, &mut delta) {
                        return Err(StreamError { message: err, retryable: false });
                    }
                    self.emit_delta(&mut delta, &mut result);
                }
                http::HttpMsg::End { code, body } => {
                    if let Some(code) = code {
                        if code >= 400 {
                            return Err(StreamError {
                                message: format!("HTTP {code}: {}", llm::error_from_body(&body)),
                                retryable: code >= 500 || code == 429,
                            });
                        }
                    } else if result.text.is_empty() && result.calls.is_empty() {
                        return Err(StreamError {
                            message: format!(
                                "connection failed: {}",
                                llm::error_from_body(&body)
                            ),
                            retryable: true,
                        });
                    }
                    break;
                }
            }
        }
        wire.finish(&mut delta);
        self.emit_delta(&mut delta, &mut result);
        if result.text.is_empty() && result.calls.is_empty() && delta.finish.is_none() {
            return Err(StreamError {
                message: "the model returned an empty reply".into(),
                retryable: true,
            });
        }
        Ok(result)
    }

    fn emit_delta(&self, delta: &mut Delta, result: &mut TurnResult) {
        let text = std::mem::take(&mut delta.text);
        if !text.is_empty() {
            result.text.push_str(&text);
            emit(&self.tx, AgentEvent::Token(text));
        }
        if !delta.reasoning.is_empty() {
            emit(&self.tx, AgentEvent::Reasoning(std::mem::take(&mut delta.reasoning)));
        }
        for call in delta.calls.drain(..) {
            result.calls.push(call);
        }
        if let Some(usage) = delta.usage.take() {
            result.usage = usage;
        }
    }

    fn account(&mut self, usage: &Usage) {
        if usage.input > 0 {
            self.session_in = usage.input;
        }
        self.session_out += usage.output;
        match cost::estimate(&self.model, usage) {
            Some(extra) => self.cost = Some(self.cost.unwrap_or(0.0) + extra),
            // An unpriced model reports no cost rather than a fake one.
            None => self.cost = None,
        }
        emit(
            &self.tx,
            AgentEvent::Usage {
                tokens_in: self.session_in,
                tokens_out: self.session_out,
                cost: self.cost,
            },
        );
    }

    // -- compaction -------------------------------------------------------

    fn estimate_tokens(&self) -> u64 {
        let per_msg: u64 = self
            .messages
            .iter()
            .map(|m| {
                let calls: usize = m.calls.iter().map(|c| c.args.len() + c.name.len()).sum();
                ((m.text.len() + calls) as u64) / 4 + 16
            })
            .sum();
        per_msg + self.ctx_max / 20
    }

    fn maybe_compact(&mut self) -> bool {
        if self.messages.len() < 8 {
            return false;
        }
        let estimate = self.estimate_tokens();
        if estimate as f64 > self.ctx_max as f64 * COMPACT_AT {
            emit(
                &self.tx,
                AgentEvent::Notice(format!(
                    "context near full ({estimate} of {} tokens), summarizing",
                    self.ctx_max
                )),
            );
            return self.compact_now();
        }
        false
    }

    /// Summarize everything but the last few messages into one message.
    fn compact_now(&mut self) -> bool {
        let keep = 4usize;
        if self.messages.len() <= keep + 1 {
            return false;
        }
        let split = self.messages.len() - keep;
        let mut transcript = String::new();
        for m in &self.messages[..split] {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            transcript.push_str(&format!("{role}: {}\n", one_line(&m.text, 600)));
            for call in &m.calls {
                transcript.push_str(&format!("  calls {} {}\n", call.name, one_line(&call.args, 200)));
            }
        }
        let instruction = "Summarize this conversation for a coding agent that has to continue it. \
                           Keep: decisions made, files changed and how, commands that mattered and \
                           their outcomes, open questions, and anything the user asked for. Drop \
                           pleasantries and repeated tool noise. Be dense and factual."
            .to_string();

        let system = instruction;
        let messages = vec![Msg::user(format!("Conversation so far:\n\n{transcript}"))];
        let schemas: Vec<llm::ToolSchema> = Vec::new();
        let turn = llm::Turn {
            model: &self.model,
            system: &system,
            messages: &messages,
            tools: &schemas,
            max_tokens: 2_048,
        };
        // A summary is a normal turn with the tools switched off, and its
        // tokens are ignored: the status bar should show the live session.
        let (kind, provider) = llm::relay::route(&self.provider, &self.model);
        let mut wire: Box<dyn Wire> = llm::wire_for(kind);
        let request = wire.build(&provider, &turn);
        let Ok(rx) = http::post(request, self.cancel.clone()) else { return false };
        let mut delta = Delta::default();
        let mut summary = String::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(120)) {
                Ok(http::HttpMsg::Line(line)) => {
                    let Some(payload) = sse_payload(&line) else { continue };
                    if payload.trim().is_empty() {
                        continue;
                    }
                    if wire.parse(&payload, &mut delta).is_err() {
                        break;
                    }
                    summary.push_str(&std::mem::take(&mut delta.text));
                }
                Ok(http::HttpMsg::End { .. }) => break,
                Err(_) => break,
            }
            if self.cancel.cancelled() {
                return false;
            }
        }
        if summary.trim().is_empty() {
            return false;
        }
        let mut kept = self.messages.split_off(split);
        let mut compacted = vec![Msg::user(format!("{COMPACT_PREFIX}\n\n{}", summary.trim()))];
        compacted.append(&mut kept);
        self.messages = compacted;
        true
    }
}

#[derive(Default)]
struct TurnResult {
    text: String,
    calls: Vec<ToolCall>,
    usage: Usage,
}

struct StreamError {
    message: String,
    retryable: bool,
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max {
        return flat;
    }
    flat.chars().take(max).collect::<String>() + "…"
}

/// SSE payload of a line, or None for comments, events, and blanks.
pub fn sse_payload(line: &str) -> Option<String> {
    let line = line.trim_end_matches('\r');
    let rest = line.strip_prefix("data:")?;
    Some(rest.trim_start().to_string())
}

/// A context window that fits the model, when nothing better is known.
pub fn default_ctx_max(model: &str) -> u64 {
    let m = model.to_ascii_lowercase();
    if m.contains("gemini") {
        1_000_000
    } else if m.contains("claude") || m.contains("gpt-5") || m.contains("gpt-4.1") {
        200_000
    } else {
        128_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Kind;

    #[test]
    fn sse_lines_are_extracted() {
        assert_eq!(sse_payload("data: {\"a\":1}").as_deref(), Some("{\"a\":1}"));
        assert_eq!(sse_payload("data:{}").as_deref(), Some("{}"));
        assert_eq!(sse_payload(": keep-alive"), None);
        assert_eq!(sse_payload("event: message"), None);
        assert_eq!(sse_payload(""), None);
        assert_eq!(sse_payload("data: x\r").as_deref(), Some("x"));
    }

    #[test]
    fn context_defaults_track_the_family() {
        assert_eq!(default_ctx_max("gemini-2.5-pro"), 1_000_000);
        assert_eq!(default_ctx_max("claude-sonnet-4-5"), 200_000);
        assert_eq!(default_ctx_max("ling-3.0-flash-fin-free"), 128_000);
    }

    #[test]
    fn one_line_flattens_and_bounds() {
        assert_eq!(one_line("a\nb", 10), "a b");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }

    #[test]
    fn agent_starts_and_stops_cleanly() {
        let dir = crate::tools::testutil::temp_dir("agent");
        let provider = Provider {
            name: "test".into(),
            kind: Kind::OpenAi,
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: String::new(),
            models: Vec::new(),
        };
        let agent = Agent::start(provider, "test-model".into(), dir, 128_000, Vec::new());
        assert_eq!(agent.model, "test-model");
        assert!(agent.poll().is_empty());
        drop(agent);
    }
}
