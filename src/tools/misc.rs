//! Environment checks: what this thing is running on, and whether the
//! pieces it depends on are actually there.

use super::{Ctx, Tool, shell::run_capture};
use crate::config::Config;
use crate::llm::{schema, string_prop};
use serde_json::{Value, json};

pub fn tools() -> Vec<Tool> {
    vec![
    Tool {
        name: "env_status",
        desc: "Report the environment: platform, tools, workspace, config.",
        params: schema(json!({}), &[]),
        run: env_status,
        read_only: true,
    },
    Tool {
        name: "doctor",
        desc: "Run live checks on the shell, files, git, and the network.",
        params: schema(
            json!({ "network": string_prop("Set to 'skip' to leave the network alone") }),
            &[],
        ),
        run: doctor,
        read_only: true,
    },
    ]
}

fn which(ctx: &mut Ctx, program: &str) -> bool {
    run_capture(ctx, &format!("command -v {program}"), 10_000).is_ok()
}

/// One line per fact, because this output ends up in a transcript.
pub fn status_lines(ctx: &Ctx) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("harness {}", env!("CARGO_PKG_VERSION")));
    lines.push(format!("platform {} · {}", std::env::consts::OS, std::env::consts::ARCH));
    lines.push(format!("workspace {}", ctx.root.display()));
    if let Ok(prefix) = std::env::var("PREFIX") {
        lines.push(format!("prefix {prefix}"));
    }
    lines.push(format!("config {}", Config::dir().join("config.json").display()));
    lines.push(format!("data {}", Config::data_dir().display()));
    lines.push(format!("full access {}", if ctx.full_access { "on" } else { "off" }));
    lines
}

fn env_status(ctx: &mut Ctx, _args: &Value) -> Result<String, String> {
    let mut out = status_lines(ctx);
    for program in ["sh", "git", "curl", "rg", "python3", "node", "jq"] {
        out.push(format!("{program} {}", if which(ctx, program) { "yes" } else { "no" }));
    }
    if let Ok(shell) = std::env::var("SHELL") {
        out.push(format!("$SHELL {shell}"));
    }
    Ok(out.join("\n"))
}

struct Check {
    name: &'static str,
    result: Result<String, String>,
}

fn doctor(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let skip_net = args.get("network").and_then(|n| n.as_str()) == Some("skip");
    let mut checks: Vec<Check> = Vec::new();

    checks.push(Check {
        name: "curl",
        result: if crate::llm::http::curl_available() {
            Ok("present".into())
        } else {
            Err("curl is not installed (pkg install curl)".into())
        },
    });
    checks.push(Check {
        name: "shell",
        result: run_capture(ctx, "echo ok", 10_000).map(|s| s.trim().to_string()),
    });
    checks.push(Check {
        name: "workspace",
        result: match std::fs::metadata(&ctx.root) {
            Ok(_) => Ok(format!("{} is readable", ctx.root.display())),
            Err(e) => Err(e.to_string()),
        },
    });
    checks.push(Check {
        name: "write",
        result: write_check(ctx),
    });
    checks.push(Check {
        name: "git",
        result: if which(ctx, "git") {
            run_capture(ctx, "git --version", 20_000).map(|s| s.trim().to_string())
        } else {
            Err("git is not installed".into())
        },
    });
    checks.push(Check {
        name: "network",
        result: if skip_net {
            Ok("skipped".into())
        } else {
            run_capture(
                ctx,
                "curl -sS -o /dev/null -w '%{http_code}' --max-time 20 https://raw.githubusercontent.com/",
                30_000,
            )
            .map(|s| format!("github returned {}", s.trim()))
        },
    });

    let failed = checks.iter().filter(|c| c.result.is_err()).count();
    let mut out = String::new();
    for check in &checks {
        match &check.result {
            Ok(detail) => out.push_str(&format!("✓ {} · {detail}\n", check.name)),
            Err(detail) => out.push_str(&format!("✗ {} · {detail}\n", check.name)),
        }
    }
    out.push_str(&format!(
        "\n{} of {} checks passed",
        checks.len() - failed,
        checks.len()
    ));
    Ok(out)
}

/// Prove the workspace is writable without leaving a file behind.
fn write_check(ctx: &Ctx) -> Result<String, String> {
    let path = ctx.root.join(".harness").join("doctor.tmp");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, b"ok").map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&path);
    Ok(format!("wrote {size} bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http;
    use crate::tools::testutil::temp_dir;

    #[test]
    fn status_mentions_the_workspace() {
        let dir = temp_dir("env");
        let ctx = Ctx::new(dir.clone(), http::Cancel::new(), "test".into());
        let lines = status_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("workspace")));
        assert!(lines.iter().any(|l| l.contains(&dir.display().to_string())));
    }

    #[test]
    fn doctor_reports_every_check() {
        let dir = temp_dir("doctor");
        let mut ctx = Ctx::new(dir.clone(), http::Cancel::new(), "test".into());
        ctx.full_access = true;
        let out = doctor(&mut ctx, &json!({"network": "skip"})).unwrap();
        assert!(out.contains("shell"), "{out}");
        assert!(out.contains("write · wrote 2 bytes"), "{out}");
        assert!(out.contains("network · skipped"), "{out}");
        assert!(out.contains("checks passed"));
        assert!(!dir.join(".harness/doctor.tmp").exists(), "the probe cleans up");
    }
}
