//! HTTP through curl.
//!
//! Termux ships curl, and borrowing it keeps TLS, proxies, and certificate
//! stores out of the binary: the release build stays a small static ELF with
//! no C dependencies, which is what makes the one-line install possible.
//!
//! Requests are streamed: curl's stdout is read line by line on a worker
//! thread and forwarded as they arrive, so tokens show up while the model is
//! still talking. Cancellation kills the child, which drops the connection.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Marker curl prints after the body, carrying the status code.
const STATUS_MARK: &str = "\u{1}HTTP\u{1}";

pub enum HttpMsg {
    /// One line of the response body, without its newline.
    Line(String),
    End {
        code: Option<u16>,
        /// The body, kept only when the request failed, for the error text.
        body: String,
    },
}

#[derive(Clone, Default)]
pub struct HttpReq {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// A handle that can stop a running request. Cheap to clone, safe to kill
/// from the UI thread while the worker reads.
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.child.lock() {
            if let Some(child) = slot.as_mut() {
                let _ = child.kill();
            }
        }
    }

    pub fn reset(&self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

pub fn curl_available() -> bool {
    Command::new("curl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Start a POST and return the receiver of body lines.
pub fn post(req: HttpReq, cancel: Cancel) -> Result<Receiver<HttpMsg>, String> {
    let mut cmd = Command::new("curl");
    cmd.arg("-sS")
        .arg("-N")
        .arg("--no-buffer")
        .arg("-X")
        .arg("POST")
        .arg("--connect-timeout")
        .arg("30")
        .arg("--data-binary")
        .arg("@-")
        .arg("-w")
        .arg(format!("\n{STATUS_MARK}%{{http_code}}\n"))
        .arg(&req.url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &req.headers {
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }

    let mut child = cmd.spawn().map_err(|e| format!("curl: {e}"))?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().ok_or("curl: no stdout")?;
    if let Ok(mut slot) = cancel.child.lock() {
        *slot = Some(child);
    }
    let child_slot = cancel.child.clone();

    if let Some(mut pipe) = stdin.take() {
        let body = req.body.clone();
        // A separate writer: curl reads the body while we read its output.
        thread::spawn(move || {
            let _ = pipe.write_all(body.as_bytes());
            let _ = pipe.flush();
        });
    }

    let (tx, rx) = channel();
    let cancel_flag = cancel.flag.clone();
    thread::spawn(move || {
        let mut raw_body = String::new();
        let mut code: Option<u16> = None;
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(rest) = line.strip_prefix(STATUS_MARK) {
                code = rest.trim().parse().ok();
                continue;
            }
            if raw_body.len() < 4096 {
                raw_body.push_str(&line);
                raw_body.push('\n');
            }
            if tx.send(HttpMsg::Line(line)).is_err() {
                break;
            }
        }
        let mut child = child_slot.lock().ok().and_then(|mut s| s.take());
        if let Some(child) = child.as_mut() {
            let _ = child.wait();
        }
        let body =
            if code.is_some_and(|c| c < 400) { String::new() } else { raw_body.trim().to_string() };
        let _ = tx.send(HttpMsg::End { code, body });
        if cancel_flag.load(Ordering::SeqCst) {
            // The caller already knows it cancelled; nothing more to say.
        }
    });
    Ok(rx)
}

/// A blocking GET: for the small calls that fill a picker, where waiting a
/// second matters less than keeping the streaming path simple.
pub fn get(
    url: &str,
    headers: &[(String, String)],
    cancel: Cancel,
    timeout_ms: u64,
) -> Result<String, String> {
    let mut cmd = Command::new("curl");
    cmd.arg("-sS")
        .arg("-L")
        .arg("--max-time")
        .arg((timeout_ms / 1000).max(1).to_string())
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in headers {
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    let mut child = cmd.spawn().map_err(|e| format!("curl: {e}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Ok(mut slot) = cancel.child.lock() {
        *slot = Some(child);
    }
    // Drain both pipes on their own threads: a full pipe would otherwise
    // stall the process we are waiting on.
    let (out_tx, out_rx) = channel::<String>();
    let (err_tx, err_rx) = channel::<String>();
    if let Some(pipe) = stdout {
        thread::spawn(move || {
            let _ = out_tx.send(read_all(pipe));
        });
    }
    if let Some(pipe) = stderr {
        thread::spawn(move || {
            let _ = err_tx.send(read_all(pipe));
        });
    }

    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms + 1_000);
    // The lock is taken per poll, never across the wait: cancelling from the
    // UI thread has to stay instant while this is running.
    let status = loop {
        let mut done = None;
        {
            let mut slot = cancel.child.lock().map_err(|_| "curl: poisoned lock".to_string())?;
            let Some(child) = slot.as_mut() else { return Err("curl: lost the process".into()) };
            match child.try_wait() {
                Ok(Some(status)) => done = Some(status),
                Ok(None) => {}
                Err(e) => return Err(e.to_string()),
            }
            if done.is_none()
                && (cancel.cancelled() || std::time::Instant::now() > deadline)
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err("cancelled".into());
            }
        }
        if let Some(status) = done {
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let body = out_rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let err = err_rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    if !status.success() {
        let message = err.trim();
        return Err(if message.is_empty() { "request failed".into() } else { message.into() });
    }
    Ok(body)
}

fn read_all(pipe: impl std::io::Read) -> String {
    let mut buf = Vec::new();
    let mut pipe = pipe;
    let _ = pipe.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_observable() {
        let c = Cancel::new();
        assert!(!c.cancelled());
        c.cancel();
        assert!(c.cancelled());
        c.reset();
        assert!(!c.cancelled());
    }

    #[test]
    fn status_marker_is_parseable() {
        let line = format!("{STATUS_MARK}200");
        assert_eq!(line.strip_prefix(STATUS_MARK).and_then(|s| s.trim().parse::<u16>().ok()), Some(200));
    }
}
