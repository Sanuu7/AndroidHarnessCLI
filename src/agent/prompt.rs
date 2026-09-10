//! The system prompt.
//!
//! Terminal-shaped: the same rules the app gives its agent, minus the phone
//! app parts, and with the width of a phone screen kept in mind.

use crate::config::Config;
use crate::tools::mem;
use std::path::Path;

pub fn build(root: &Path, extra: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "You are Android Harness, a coding agent running in a terminal on an Android phone.\n\
         Workspace: {}\n\n",
        root.display()
    ));
    out.push_str(
        "Rules:\n\
         - Tool paths are relative to the workspace root. Paths that escape it are refused.\n\
         - Explore before you change anything: list_dir, grep, read_file.\n\
         - Prefer edit_file for targeted changes, multi_edit for several in one file, write_file \
         for new files.\n\
         - Use todo_write to track multi-step work and keep the statuses current.\n\
         - Run commands with shell; use shell_background for servers and watches.\n\
         - Lead with the result, not the process. Say what changed and what you checked.\n\
         - Ground every claim in tool output you actually saw. Never say a test or build passed \
         unless the corresponding command succeeded. If something failed or you did not run it, \
         say so plainly.\n\
         - Never invent file contents you have not read.\n\
         - When a tool fails, read the error and change your inputs instead of repeating the call.\n\
         - Text files end with a newline. Keep that when you edit.\n\
         - This is a phone screen: answer in short lines and flat bullet lists under about 40 \
         columns. No tables and no ASCII boxes.\n\
         - If a decision is genuinely the user's, ask instead of guessing.\n",
    );

    let agents = read_doc(root, "AGENTS.md").or_else(|| read_doc(root, "HARNESS.md"));
    if let Some(text) = agents {
        out.push_str("\n# AGENTS.md (project instructions)\n");
        out.push_str(&text);
        out.push('\n');
    }

    let notes = read_doc(root, mem::NOTES);
    if let Some(text) = notes.filter(|t| !t.trim().is_empty()) {
        out.push_str("\n# Notes from earlier sessions\n");
        out.push_str(&text);
        out.push('\n');
    }

    let ctx_root = crate::tools::Ctx::new(
        root.to_path_buf(),
        crate::llm::http::Cancel::new(),
        "prompt".into(),
    );
    let catalog = mem::catalog(&ctx_root);
    if !catalog.is_empty() {
        out.push_str("\n# Skills\n");
        out.push_str(&catalog);
    }

    let todos = mem::read_todos(root);
    if !todos.is_empty() {
        out.push_str("\n# Current todo list\n");
        for (content, status) in todos {
            let mark = match status.as_str() {
                "done" => "x",
                "in_progress" => "~",
                _ => " ",
            };
            out.push_str(&format!("- [{mark}] {content}\n"));
        }
    }

    if !extra.is_empty() {
        out.push_str("\n# Preferences\n");
        for line in extra {
            out.push_str(&format!("- {line}\n"));
        }
    }

    let home = Config::dir().parent().map(|p| p.display().to_string()).unwrap_or_default();
    out.push_str(&format!(
        "\nEnvironment: Rust binary on {} ({}), no runtime dependencies. Skills and config live \
         under {home}. Explain install steps in Termux terms (pkg install ...).\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    out
}

fn read_doc(root: &Path, name: &str) -> Option<String> {
    let path = root.join(name);
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out: String = trimmed.chars().take(12_000).collect();
    if trimmed.chars().count() > 12_000 {
        out.push_str("\n… truncated");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testutil::temp_dir;
    use std::fs;

    #[test]
    fn prompt_carries_the_rules_and_the_workspace() {
        let dir = temp_dir("prompt");
        let text = build(&dir, &[]);
        assert!(text.contains("Android Harness"));
        assert!(text.contains(&dir.display().to_string()));
        assert!(text.contains("edit_file"));
        assert!(text.contains("phone screen"), "the width rule matters");
    }

    #[test]
    fn agents_md_is_inlined() {
        let dir = temp_dir("prompt-agents");
        fs::write(dir.join("AGENTS.md"), "Always run cargo fmt.").unwrap();
        let text = build(&dir, &[]);
        assert!(text.contains("Always run cargo fmt."));
    }

    #[test]
    fn todos_show_up_with_marks() {
        let dir = temp_dir("prompt-todos");
        fs::create_dir_all(dir.join(".harness")).unwrap();
        fs::write(
            dir.join(".harness/todos.json"),
            r#"[{"content":"ship it","status":"in_progress"},{"content":"write docs","status":"done"}]"#,
        )
        .unwrap();
        let text = build(&dir, &[]);
        assert!(text.contains("- [~] ship it"));
        assert!(text.contains("- [x] write docs"));
    }

    #[test]
    fn preferences_are_appended() {
        let dir = temp_dir("prompt-prefs");
        let text = build(&dir, &["no em-dashes".to_string()]);
        assert!(text.contains("- no em-dashes"));
    }
}
