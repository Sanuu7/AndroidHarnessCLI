//! A fake agent that drives the UI until the real engine lands.
//!
//! Same shape as the real thing will be: a worker on its own thread pushing
//! events through a channel while the UI keeps drawing at 30fps. Scripts are
//! declarative so the demo reads like a session transcript.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread;
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Thinking,
    Token(String),
    ToolStart { name: String, args: String },
    ToolDone { name: String, output: String, ok: bool, ms: u64 },
    Usage { tokens_in: u64, tokens_out: u64, cost: f64 },
    Cancelled,
    Done,
}

enum Step {
    Think(u64),
    Say(&'static str),
    Tool(&'static str, &'static str, &'static str, u64),
    Usage(u64, u64, f64),
}

// Statics, not consts: `script_for` returns references and tests compare them.
static GREETING: &[Step] = &[
    Step::Think(600),
    Step::Say(
        "I'll start with a quick look at the workspace.\n\nLooking at the auth flow, the token \
         check uses `<=` instead of `<`, so an expired token passes validation for one extra \
         second.\n\nPlan:\n\n- tighten the comparison in `TokenValidator::validate`\n- add a \
         regression test for the boundary\n- re-run the auth suite",
    ),
    Step::Tool(
        "bash",
        "git status --short",
        " M app/src/auth/TokenValidator.kt\n?? app/src/test/TokenValidatorTest.kt",
        42,
    ),
    Step::Tool(
        "read_file",
        "app/src/auth/TokenValidator.kt",
        "fun validate(token: Token, now: Long): Boolean {\n    val expiry = token.claims.expiry\n    \
         if (now <= expiry) return true\n    return false\n}",
        12,
    ),
    Step::Think(700),
    Step::Say(
        "Found it. The boundary is off by one:\n\n```diff\n- if (now <= expiry) return true\n+ if \
         (now < expiry) return true\n```\n\nApplied the fix, added the regression test, and the \
         auth suite is green.",
    ),
    Step::Tool(
        "bash",
        "gradle :app:testDebugUnitTest --tests '*TokenValidatorTest*'",
        "> Task :app:testDebugUnitTest\n\n3 tests completed, 3 passed",
        8_400,
    ),
    Step::Usage(12_480, 1_164, 0.0031),
];

static DOCTOR: &[Step] = &[
    Step::Think(400),
    Step::Say("Running the doctor. Each tool family gets one live check."),
    Step::Tool("write_file", "doctor/unicode.txt", "wrote 34 bytes", 6),
    Step::Tool("read_file", "doctor/unicode.txt", "café 中文 ✅\nwith tabs\there", 4),
    Step::Tool("bash", "printf 'a\\na\\nunique\\na' | wc -c", "8", 11),
    Step::Tool("apply_patch", "nl.txt", "patched 1 hunk", 9),
    Step::Tool("web_fetch", "https://example.com", "200 OK · 1.2 KB", 640),
    Step::Say(
        "6 of 6 checks passed:\n\n- file crud and unicode round trip\n- sandbox boundaries blocked \
         every escape attempt\n- patch atomicity on a newline-less file\n- network fetch returned \
         200",
    ),
    Step::Usage(3_100, 402, 0.0008),
];

static LONG: &[Step] = &[
    Step::Think(500),
    Step::Say(
        "# Where the time goes\n\nThe app spends most of its startup budget on three things.\n\n## \
         1. Catalog parsing\n\nThe models.dev catalog is 2.4 MB of JSON and gets parsed on every \
         cold start. Caching the parsed form brings first paint down by half.\n\n## 2. Database \
         migrations\n\nRoom runs nine migrations before the first frame. Most of them touch empty \
         tables and could be skipped when the schema version already matches.\n\n```kotlin\nif \
         (db.version == Target) return  // nothing to do\n```\n\n## 3. Compose recomposition\n\nThe \
         transcript recomposes every message on each token. Keying by message id and hoisting the \
         streaming text into its own composable fixes it.\n\n> The compiler metrics show 41 \
         recompositions per streamed token before the change, 1 after.\n\nNext steps:\n\n- cache \
         the parsed catalog next to the raw payload\n- short circuit migrations on matching \
         versions\n- measure with the macrobenchmark module",
    ),
    Step::Usage(18_200, 2_960, 0.0074),
];

fn script_for(prompt: &str) -> &'static [Step] {
    let p = prompt.to_lowercase();
    if p.contains("__doctor") {
        DOCTOR
    } else if p.contains("long") || p.contains("slow") || p.contains("startup") {
        LONG
    } else {
        GREETING
    }
}

pub struct Demo {
    prompts: Option<Sender<String>>,
    events: Receiver<AgentEvent>,
    cancel: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Demo {
    pub fn start() -> Self {
        let (ptx, prx) = channel::<String>();
        let (etx, erx) = channel::<AgentEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = {
            let cancel = cancel.clone();
            thread::spawn(move || {
                while let Ok(prompt) = prx.recv() {
                    cancel.store(false, Ordering::SeqCst);
                    run(script_for(&prompt), &etx, &cancel);
                    if cancel.load(Ordering::SeqCst) {
                        let _ = etx.send(AgentEvent::Cancelled);
                    } else {
                        let _ = etx.send(AgentEvent::Done);
                    }
                }
            })
        };
        Self {
            prompts: Some(ptx),
            events: erx,
            cancel,
            worker: Some(worker),
        }
    }

    pub fn send(&self, prompt: &str) {
        if let Some(tx) = &self.prompts {
            let _ = tx.send(prompt.to_string());
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
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

impl Drop for Demo {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.prompts = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn run(steps: &[Step], tx: &Sender<AgentEvent>, cancel: &Arc<AtomicBool>) {
    for step in steps {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        match step {
            Step::Think(ms) => {
                let _ = tx.send(AgentEvent::Thinking);
                if !sleep(*ms, cancel) {
                    return;
                }
            }
            Step::Say(text) => {
                for chunk in chunks(text) {
                    if !sleep(16, cancel) {
                        return;
                    }
                    let _ = tx.send(AgentEvent::Token(chunk));
                }
            }
            Step::Tool(name, args, out, ms) => {
                let _ = tx.send(AgentEvent::ToolStart {
                    name: name.to_string(),
                    args: args.to_string(),
                });
                let took = (*ms).max(1);
                let shown = took.min(1_600);
                if !sleep(shown, cancel) {
                    return;
                }
                let _ = tx.send(AgentEvent::ToolDone {
                    name: name.to_string(),
                    output: out.to_string(),
                    ok: true,
                    ms: took,
                });
            }
            Step::Usage(inp, out, cost) => {
                let _ = tx.send(AgentEvent::Usage {
                    tokens_in: *inp,
                    tokens_out: *out,
                    cost: *cost,
                });
            }
        }
    }
}

/// Slice text into token-ish chunks so streaming looks like a real model.
fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut n = 0;
    for (i, c) in text.chars().enumerate() {
        cur.push(c);
        n += 1;
        let boundary = c == ' ' || c == '\n' || c == '-';
        let size = 2 + (i % 3);
        if n >= size || boundary {
            out.push(std::mem::take(&mut cur));
            n = 0;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Sleep in small slices so cancellation stays responsive.
fn sleep(ms: u64, cancel: &Arc<AtomicBool>) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(ms) {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
    !cancel.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_cover_prompts() {
        assert!(std::ptr::eq(script_for("__doctor"), DOCTOR));
        assert!(std::ptr::eq(script_for("this is LONG please"), LONG));
        assert!(std::ptr::eq(script_for("hi"), GREETING));
    }

    #[test]
    fn chunks_reassemble() {
        let text = "hello **world** and more";
        let joined: String = chunks(text).concat();
        assert_eq!(joined, text);
    }

    #[test]
    fn empty_script_emits_nothing() {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        run(&[], &tx, &cancel);
        assert!(rx.try_recv().is_err());
    }
}
