//! Git, as thin wrappers over the shell. Reading is cheap; anything that
//! writes says exactly what it did.

use super::shell::run_capture;
use super::{Ctx, Tool, bool_arg, int_arg, str_arg, str_req};
use crate::llm::{bool_prop, number_prop, schema, string_prop};
use serde_json::{Value, json};

pub fn tools() -> Vec<Tool> {
    vec![
    Tool {
        name: "git_status",
        desc: "Branch and working tree state.",
        params: schema(json!({}), &[]),
        run: git_status,
        read_only: true,
    },
    Tool {
        name: "git_diff",
        desc: "Diff of unstaged changes, or staged ones with staged=true.",
        params: schema(
            json!({
                "path": string_prop("Limit the diff to this path"),
                "staged": bool_prop("Show staged changes instead"),
                "stat": bool_prop("Summary only"),
            }),
            &[],
        ),
        run: git_diff,
        read_only: true,
    },
    Tool {
        name: "git_log",
        desc: "Recent commits.",
        params: schema(
            json!({
                "n": number_prop("How many commits, default 15"),
                "path": string_prop("Limit to this path"),
            }),
            &[],
        ),
        run: git_log,
        read_only: true,
    },
    Tool {
        name: "git_commit",
        desc: "Stage changes and commit them.",
        params: schema(
            json!({
                "message": string_prop("Commit message"),
                "paths": json!({
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Stage only these paths, default everything",
                }),
                "amend": bool_prop("Amend the previous commit"),
            }),
            &["message"],
        ),
        run: git_commit,
        read_only: false,
    },
    Tool {
        name: "git_push",
        desc: "Push the current branch, setting upstream on the first push.",
        params: schema(
            json!({
                "remote": string_prop("Remote, default origin"),
                "branch": string_prop("Branch, default the current one"),
                "set_upstream": bool_prop("Force -u"),
            }),
            &[],
        ),
        run: git_push,
        read_only: false,
    },
    Tool {
        name: "git_pull",
        desc: "Pull the current branch.",
        params: schema(json!({ "remote": string_prop("Remote, default origin") }), &[]),
        run: git_pull,
        read_only: false,
    },
    ]
}

fn git(ctx: &mut Ctx, args: &str) -> Result<String, String> {
    run_capture(ctx, &format!("git {args}"), 120_000)
}

fn git_status(ctx: &mut Ctx, _args: &Value) -> Result<String, String> {
    git(ctx, "status --short --branch")
}

fn git_diff(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let mut cmd = String::from("diff");
    if bool_arg(args, "staged") {
        cmd.push_str(" --staged");
    }
    if bool_arg(args, "stat") {
        cmd.push_str(" --stat");
    }
    if let Some(path) = str_arg(args, "path") {
        cmd.push_str(&format!(" -- {}", quote(&path)));
    }
    git(ctx, &cmd)
}

fn git_log(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let n = int_arg(args, "n").unwrap_or(15).clamp(1, 200);
    let mut cmd = format!("log --oneline --decorate -n {n}");
    if let Some(path) = str_arg(args, "path") {
        cmd.push_str(&format!(" -- {}", quote(&path)));
    }
    git(ctx, &cmd)
}

fn git_commit(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let message = str_req(args, "message")?;
    let amend = bool_arg(args, "amend");
    let paths: Vec<String> = args
        .get("paths")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let stage = if paths.is_empty() {
        "add -A".to_string()
    } else {
        format!("add -- {}", paths.iter().map(|p| quote(p)).collect::<Vec<_>>().join(" "))
    };
    let mut cmd = String::new();
    if !amend {
        cmd.push_str(&format!("git {stage} && "));
    }
    cmd.push_str(&format!("git commit {}", if amend { "--amend" } else { "" }));
    cmd.push_str(&format!(" -m {}", quote(&message)));
    run_capture(ctx, &cmd, 120_000)
}

/// The upstream a branch pushes to, if it has one. Mirrors the app's fix: a
/// branch that merely *exists* on the remote is not an upstream.
fn has_upstream(ctx: &mut Ctx, branch: &str) -> bool {
    let remote = run_capture(
        ctx,
        &format!("git rev-parse --abbrev-ref --symbolic-full-name {}@{{u}}", quote(branch)),
        20_000,
    )
    .map(|s| s.trim().to_string())
    .unwrap_or_default();
    if remote.is_empty() || remote.contains("fatal") {
        return false;
    }
    // And the merge ref has to point at this branch, or a push would go to
    // the wrong place on the next pull.
    let merge = run_capture(
        ctx,
        &format!("git config --get branch.{}.merge", shell_word(branch)),
        20_000,
    )
    .map(|s| s.trim().to_string())
    .unwrap_or_default();
    merge == format!("refs/heads/{branch}")
}

fn shell_word(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect()
}

fn git_push(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let remote = str_arg(args, "remote").unwrap_or_else(|| "origin".into());
    let branch = match str_arg(args, "branch") {
        Some(b) => b,
        None => run_capture(ctx, "git rev-parse --abbrev-ref HEAD", 20_000)?
            .trim()
            .to_string(),
    };
    let force_upstream = bool_arg(args, "set_upstream");
    let upstream = if force_upstream { false } else { has_upstream(ctx, &branch) };
    let flag = if upstream { "" } else { " -u" };
    let out = run_capture(
        ctx,
        &format!("git push{flag} {} {}", quote(&remote), quote(&branch)),
        300_000,
    )?;
    if upstream {
        Ok(out)
    } else {
        Ok(format!("{out}\n(upstream set to {remote}/{branch})"))
    }
}

fn git_pull(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let remote = str_arg(args, "remote").unwrap_or_else(|| "origin".into());
    git(ctx, &format!("pull {}", quote(&remote)))
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testutil::temp_dir;

    fn git_ctx(dir: &std::path::Path) -> Ctx {
        let mut ctx = Ctx::new(dir.to_path_buf(), crate::llm::http::Cancel::new(), "test".into());
        ctx.full_access = true;
        let run = |c: &mut Ctx, cmd: &str| crate::tools::shell::run_capture(c, cmd, 60_000);
        run(&mut ctx, "git init -q -b main").unwrap();
        run(&mut ctx, "git config user.email harness@test").unwrap();
        run(&mut ctx, "git config user.name harness").unwrap();
        run(&mut ctx, "git config commit.gpgsign false").unwrap();
        ctx
    }

    #[test]
    fn status_then_commit_then_log() {
        let dir = temp_dir("git");
        let mut ctx = git_ctx(&dir);
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        let status = git_status(&mut ctx, &json!({})).unwrap();
        assert!(status.contains("main"), "{status}");
        assert!(status.contains("a.txt"));
        let out = git_commit(&mut ctx, &json!({"message": "Add a file"})).unwrap();
        assert!(out.contains("Add a file"), "{out}");
        let log = git_log(&mut ctx, &json!({"n": 3})).unwrap();
        assert!(log.contains("Add a file"));
        let diff = git_diff(&mut ctx, &json!({"staged": true})).unwrap();
        assert!(!diff.contains("a.txt"), "nothing left staged after the commit");
    }

    #[test]
    fn upstream_detection_is_honest() {
        let dir = temp_dir("git-upstream");
        let mut ctx = git_ctx(&dir);
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        git_commit(&mut ctx, &json!({"message": "one"})).unwrap();
        assert!(!has_upstream(&mut ctx, "main"), "a fresh branch has no upstream");
    }

    #[test]
    fn branch_names_are_sanitized_for_config_lookup() {
        assert_eq!(shell_word("feat/x y"), "featxy");
        assert_eq!(shell_word("main"), "main");
    }
}
