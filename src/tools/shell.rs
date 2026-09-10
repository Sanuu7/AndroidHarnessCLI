//! Shell: one command at a time, plus background jobs.
//!
//! Output goes to a scratch file rather than a pipe, so a command that prints
//! a gigabyte cannot deadlock us, and cancelling is just a kill.

use super::{Ctx, Tool, int_arg, str_req};
use crate::config::Config;
use crate::llm::{number_prop, schema, string_prop};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub fn tools() -> Vec<Tool> {
    vec![
    Tool {
        name: "shell",
        desc: "Run a shell command in the workspace and return its output.",
        params: schema(
            json!({
                "command": string_prop("Command line, run with sh -c"),
                "cwd": string_prop("Working directory inside the workspace"),
                "timeout_ms": number_prop("Kill after this long, default 120000"),
            }),
            &["command"],
        ),
        run: shell,
        read_only: false,
    },
    Tool {
        name: "shell_background",
        desc: "Start a long-running command (a server, a watch) and return its job id.",
        params: schema(
            json!({
                "command": string_prop("Command line, run with sh -c"),
                "cwd": string_prop("Working directory inside the workspace"),
            }),
            &["command"],
        ),
        run: shell_background,
        read_only: false,
    },
    Tool {
        name: "bg_list",
        desc: "List background jobs with the tail of their output.",
        params: schema(json!({}), &[]),
        run: bg_list,
        read_only: true,
    },
    Tool {
        name: "bg_kill",
        desc: "Stop a background job.",
        params: schema(json!({ "id": number_prop("Job id from bg_list") }), &["id"]),
        run: bg_kill,
        read_only: false,
    },
    ]
}

/// Scratch space for command output. Private: it can hold anything.
fn scratch() -> PathBuf {
    let dir = Config::data_dir().join("scratch");
    let _ = fs::create_dir_all(&dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    dir
}

fn log_path(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    // The session id in the name makes it obvious where a stray file came
    // from, and keeps two sessions from fighting over one scratch file.
    let tag: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    scratch().join(format!("{}-{}-{n}.log", &tag[..tag.len().min(24)], std::process::id()))
}

/// Run a command to completion, returning its combined output.
pub fn run_capture(ctx: &mut Ctx, command: &str, timeout_ms: u64) -> Result<String, String> {
    let cwd = ctx.root.clone();
    let path = log_path("shell");
    let file = fs::File::create(&path).map_err(|e| format!("scratch: {e}"))?;
    let file2 = file.try_clone().map_err(|e| e.to_string())?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(file2))
        .process_group(0)
        .spawn()
        .map_err(|e| format!("sh: {e}"))?;
    let pid = child.id();
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(e) => {
                let _ = fs::remove_file(&path);
                return Err(format!("waiting on sh failed: {e}"));
            }
        }
        if ctx.cancel.cancelled() {
            kill_group(pid);
            let _ = child.wait();
            let _ = fs::remove_file(&path);
            return Err("cancelled".into());
        }
        if started.elapsed() > Duration::from_millis(timeout_ms) {
            kill_group(pid);
            let _ = child.wait();
            let out = read_log(&path, 4_000);
            let _ = fs::remove_file(&path);
            return Err(format!(
                "timed out after {}s. output so far:\n{}",
                timeout_ms / 1000,
                out.trim()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);
    let output = read_log(&path, super::MAX_OUTPUT);
    let _ = fs::remove_file(&path);
    let trimmed = output.trim_end();
    if code != 0 {
        let body = if trimmed.is_empty() { "(no output)" } else { trimmed };
        return Err(format!("exit {code}\n{body}"));
    }
    if trimmed.is_empty() {
        return Ok("(no output)".into());
    }
    Ok(trimmed.to_string())
}

fn read_log(path: &PathBuf, cap: usize) -> String {
    let Ok(bytes) = fs::read(path) else { return String::new() };
    let text = String::from_utf8_lossy(&bytes).to_string();
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… {} bytes dropped", &text[..end], text.len() - end)
}

fn kill_group(pid: u32) {
    // Negative pid: the whole process group, so children die with it.
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(format!("-{pid}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn shell(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let command = str_req(args, "command")?;
    let timeout = int_arg(args, "timeout_ms").unwrap_or(120_000).clamp(1_000, 3_600_000);
    if let Some(cwd) = args.get("cwd").and_then(|c| c.as_str()) {
        ctx.root = ctx.resolve(cwd)?;
    }
    run_capture(ctx, &command, timeout)
}

// -- background jobs -------------------------------------------------------

struct Job {
    id: u32,
    command: String,
    pid: u32,
    child: Child,
    log: PathBuf,
    started: Instant,
}

fn jobs() -> &'static Mutex<Vec<Job>> {
    static JOBS: OnceLock<Mutex<Vec<Job>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(Vec::new()))
}

fn shell_background(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let command = str_req(args, "command")?;
    let cwd = match args.get("cwd").and_then(|c| c.as_str()) {
        Some(dir) => ctx.resolve(dir)?,
        None => ctx.root.clone(),
    };
    let path = log_path(&format!("bg-{}", ctx.session));
    let file = fs::File::create(&path).map_err(|e| format!("scratch: {e}"))?;
    let file2 = file.try_clone().map_err(|e| e.to_string())?;
    let child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(file2))
        .process_group(0)
        .spawn()
        .map_err(|e| format!("sh: {e}"))?;
    let pid = child.id();
    let mut list = jobs().lock().map_err(|_| "job table is poisoned")?;
    let id = list.iter().map(|j| j.id).max().unwrap_or(0) + 1;
    list.push(Job { id, command: command.clone(), pid, child, log: path, started: Instant::now() });
    Ok(format!("job {id} started (pid {pid}): {command}"))
}

fn bg_list(_ctx: &mut Ctx, _args: &Value) -> Result<String, String> {
    let mut list = jobs().lock().map_err(|_| "job table is poisoned")?;
    if list.is_empty() {
        return Ok("no background jobs".into());
    }
    let mut out = String::new();
    for job in list.iter_mut() {
        let state = match job.child.try_wait() {
            Ok(Some(status)) => format!("exited {}", status.code().unwrap_or(-1)),
            Ok(None) => format!("running {}s", job.started.elapsed().as_secs()),
            Err(_) => "unknown".into(),
        };
        let tail = read_log(&job.log, 400);
        let tail = tail.trim();
        out.push_str(&format!("{} · {} · {}\n", job.id, job.command, state));
        if !tail.is_empty() {
            for line in tail.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev() {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }
    Ok(out)
}

fn bg_kill(_ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let id = int_arg(args, "id").ok_or("id is required")? as u32;
    let mut list = jobs().lock().map_err(|_| "job table is poisoned")?;
    let Some(job) = list.iter_mut().find(|j| j.id == id) else {
        return Err(format!("no job {id}"));
    };
    kill_group(job.pid);
    let _ = job.child.kill();
    let _ = job.child.wait();
    let _ = fs::remove_file(&job.log);
    list.retain(|j| j.id != id);
    Ok(format!("job {id} stopped"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http;
    use crate::tools::testutil::temp_dir;

    fn ctx_in(dir: &std::path::Path) -> Ctx {
        let mut c = Ctx::new(dir.to_path_buf(), http::Cancel::new(), "test".into());
        c.full_access = true;
        c
    }

    #[test]
    fn captures_stdout_and_stderr() {
        let dir = temp_dir("sh");
        let mut ctx = ctx_in(&dir);
        let out = run_capture(&mut ctx, "echo one; echo two 1>&2", 10_000).unwrap();
        assert!(out.contains("one"));
        assert!(out.contains("two"));
    }

    #[test]
    fn nonzero_exit_is_an_error_with_the_output() {
        let dir = temp_dir("sh-fail");
        let mut ctx = ctx_in(&dir);
        let err = run_capture(&mut ctx, "echo boom; exit 3", 10_000).unwrap_err();
        assert!(err.starts_with("exit 3"), "{err}");
        assert!(err.contains("boom"));
    }

    #[test]
    fn runs_in_the_workspace() {
        let dir = temp_dir("sh-cwd");
        fs::write(dir.join("marker.txt"), "x").unwrap();
        let mut ctx = ctx_in(&dir);
        let out = run_capture(&mut ctx, "ls", 10_000).unwrap();
        assert!(out.contains("marker.txt"));
    }

    #[test]
    fn timeouts_kill_the_command() {
        let dir = temp_dir("sh-timeout");
        let mut ctx = ctx_in(&dir);
        let started = Instant::now();
        let err = run_capture(&mut ctx, "sleep 30", 300).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5), "kill should be prompt");
    }

    #[test]
    fn cancellation_stops_the_command() {
        let dir = temp_dir("sh-cancel");
        let cancel = http::Cancel::new();
        let mut ctx = Ctx::new(dir.clone(), cancel.clone(), "t".into());
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.cancel();
        });
        let err = run_capture(&mut ctx, "sleep 30", 60_000).unwrap_err();
        assert_eq!(err, "cancelled");
    }

    #[test]
    fn the_shell_tool_respects_cwd() {
        let dir = temp_dir("sh-tool");
        fs::create_dir_all(dir.join("sub")).unwrap();
        let mut ctx = ctx_in(&dir);
        let out = shell(&mut ctx, &json!({"command": "pwd", "cwd": "sub"})).unwrap();
        assert!(out.trim().ends_with("sub"), "{out}");
    }

    #[test]
    fn background_jobs_start_list_and_stop() {
        let dir = temp_dir("bg");
        let mut ctx = ctx_in(&dir);
        let out = shell_background(&mut ctx, &json!({"command": "echo hi; sleep 20"})).unwrap();
        assert!(out.contains("job 1"));
        std::thread::sleep(Duration::from_millis(300));
        let listed = bg_list(&mut ctx, &json!({})).unwrap();
        assert!(listed.contains("running"), "{listed}");
        let killed = bg_kill(&mut ctx, &json!({"id": 1})).unwrap();
        assert!(killed.contains("stopped"));
        assert_eq!(bg_list(&mut ctx, &json!({})).unwrap(), "no background jobs");
    }

    #[test]
    fn output_is_capped() {
        let dir = temp_dir("sh-big");
        let mut ctx = ctx_in(&dir);
        let out = run_capture(&mut ctx, "yes x | head -c 200000", 30_000).unwrap();
        assert!(out.len() <= super::super::MAX_OUTPUT + 64, "{}", out.len());
        assert!(out.contains("bytes dropped"));
    }
}
