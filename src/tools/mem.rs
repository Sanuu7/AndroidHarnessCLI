//! Things the agent remembers: notes, todos, and skills.
//!
//! All of it lives in `.harness/` inside the workspace, so a project carries
//! its own memory and nothing is hidden in a home directory.

use super::{Ctx, Tool, bool_arg, str_arg, str_req};
use crate::config::Config;
use crate::llm::{bool_prop, schema, string_prop};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

pub const DIR: &str = ".harness";
pub const NOTES: &str = ".harness/memory.md";

pub fn tools() -> Vec<Tool> {
    vec![
    Tool {
        name: "memory_read",
        desc: "Read the workspace notes, or one topic file.",
        params: schema(json!({ "topic": string_prop("Topic file name, omit for the main notes") }), &[]),
        run: memory_read,
        read_only: true,
    },
    Tool {
        name: "memory_write",
        desc: "Add to the workspace notes. Use it for facts worth keeping between sessions.",
        params: schema(
            json!({
                "content": string_prop("Text to write"),
                "topic": string_prop("Write to this topic file instead"),
                "replace": bool_prop("Replace the file instead of appending"),
            }),
            &["content"],
        ),
        run: memory_write,
        read_only: false,
    },
    Tool {
        name: "memory_search",
        desc: "Search notes and topic files.",
        params: schema(json!({ "query": string_prop("Text to find") }), &["query"]),
        run: memory_search,
        read_only: true,
    },
    Tool {
        name: "todo_write",
        desc: "Set the task list. Replaces the list each time; keep it current.",
        params: schema(
            json!({
                "todos": json!({
                    "type": "array",
                    "description": "The full list, in order",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": string_prop("What needs doing"),
                            "status": string_prop("pending, in_progress, or done"),
                        },
                        "required": ["content", "status"],
                    },
                }),
            }),
            &["todos"],
        ),
        run: todo_write,
        read_only: false,
    },
    Tool {
        name: "skill_manage",
        desc: "Create or update a workspace skill. Existing skills require overwrite=true. Use only when the user requests a reusable skill.",
        params: schema(json!({"name": string_prop("Letters, digits, hyphens or underscores; max 64 characters"), "content": string_prop("Full SKILL.md contents"), "overwrite": bool_prop("Explicitly replace an existing skill")}), &["name", "content"]),
        run: skill_manage,
        read_only: false,
    },
    Tool {
        name: "skills_list",
        desc: "List installed skills with their descriptions.",
        params: schema(json!({}), &[]),
        run: skills_list,
        read_only: true,
    },
    Tool {
        name: "skill_view",
        desc: "Read a skill's full instructions before following them.",
        params: schema(json!({ "name": string_prop("Skill name"), "file_path": string_prop("Optional supporting file inside the skill folder") }), &["name"]),
        run: skill_view,
        read_only: true,
    },
    ]
}

const SEED: &str = "# Notes\n\n";

fn notes_path(ctx: &Ctx) -> PathBuf {
    ctx.root.join(NOTES)
}

fn topics_dir(ctx: &Ctx) -> PathBuf {
    ctx.root.join(DIR).join("memory")
}

fn memory_read(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    if str_arg(args, "topic").is_none() {
        let core = fs::read_to_string(notes_path(ctx)).unwrap_or_else(|_| "(empty)".into());
        return Ok(super::clip(format!("{core}\n\nTopics: {}", memory_topics(&ctx.root).join(", "))));
    }
    let path = match str_arg(args, "topic") {
        Some(topic) => topics_dir(ctx).join(format!("{}.md", sanitize(&topic))),
        None => notes_path(ctx),
    };
    match fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => Ok(super::clip(text)),
        Ok(_) => Ok(format!("{} is empty", ctx.display(&path))),
        Err(_) => Ok(format!("nothing stored yet ({})", ctx.display(&path))),
    }
}

pub fn memory_topics(root: &Path) -> Vec<String> {
    let mut topics: Vec<String> = fs::read_dir(root.join(DIR).join("memory"))
        .into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "md"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    topics.sort();
    topics
}

fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .to_lowercase()
}

fn memory_write(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let content = str_req(args, "content")?;
    let replace = bool_arg(args, "replace");
    let path = match str_arg(args, "topic") {
        Some(topic) => {
            let dir = topics_dir(ctx);
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            dir.join(format!("{}.md", sanitize(&topic)))
        }
        None => notes_path(ctx),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let existing = fs::read_to_string(&path).unwrap_or_else(|_| SEED.to_string());
    let body = if replace {
        content.clone()
    } else {
        let mut body = existing;
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("- ");
        body.push_str(&content.replace('\n', "\n  "));
        body.push('\n');
        body
    };
    fs::write(&path, &body).map_err(|e| e.to_string())?;
    Ok(format!("wrote {} ({} bytes)", ctx.display(&path), body.len()))
}

fn memory_search(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let query = str_req(args, "query")?.to_lowercase();
    let mut hits: Vec<String> = Vec::new();
    let mut files: Vec<PathBuf> = vec![notes_path(ctx)];
    if let Ok(entries) = fs::read_dir(topics_dir(ctx)) {
        files.extend(entries.flatten().map(|e| e.path()));
    }
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else { continue };
        for (i, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&query) {
                hits.push(format!("{}:{}: {}", ctx.display(&path), i + 1, line.trim()));
            }
        }
    }
    if hits.is_empty() {
        return Ok(format!("no notes mention '{query}'"));
    }
    Ok(super::clip(hits.join("\n")))
}

/// Todos are stored as JSON so the system prompt can render them back.
pub fn todos_path(ctx: &Ctx) -> PathBuf {
    ctx.root.join(DIR).join("todos.json")
}

pub fn read_todos(root: &Path) -> Vec<(String, String)> {
    let Ok(text) = fs::read_to_string(root.join(DIR).join("todos.json")) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some((
                        t.get("content")?.as_str()?.to_string(),
                        t.get("status").and_then(|s| s.as_str()).unwrap_or("pending").to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn todo_write(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let todos = args
        .get("todos")
        .and_then(|t| t.as_array())
        .ok_or("todos must be an array")?;
    let list: Vec<Value> = todos
        .iter()
        .filter_map(|t| {
            let content = t.get("content")?.as_str()?.to_string();
            let status = t.get("status").and_then(|s| s.as_str()).unwrap_or("pending").to_string();
            Some(json!({ "content": content, "status": status }))
        })
        .collect();
    let path = todos_path(ctx);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&path, serde_json::to_string_pretty(&Value::Array(list.clone())).unwrap())
        .map_err(|e| e.to_string())?;
    let done = list
        .iter()
        .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("done"))
        .count();
    Ok(format!("{} todos ({done} done)", list.len()))
}

// -- skills ----------------------------------------------------------------

/// Skill folders the CLI looks in: the workspace first, then the user's.
pub fn skill_dirs(ctx: &Ctx) -> Vec<PathBuf> {
    vec![ctx.root.join(DIR).join("skills"), Config::dir().join("skills")]
}

pub fn list_skills(ctx: &Ctx) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    for dir in skill_dirs(ctx) {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let file = if path.is_dir() { path.join("SKILL.md") } else { path.clone() };
            if !file.exists() || file.extension().map(|e| e != "md").unwrap_or(true) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&file) else { continue };
            let name = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "skill".into());
            if !out.iter().any(|(n, _, _): &(String, String, PathBuf)| n.eq_ignore_ascii_case(&name)) {
                out.push((name, describe(&text), file));
            }
        }
    }
    out.sort();
    out
}

/// The description line: frontmatter first, then the first heading, then
/// whatever prose comes first.
pub fn describe(text: &str) -> String {
    let mut in_front = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            in_front = !in_front;
            continue;
        }
        if in_front {
            if let Some(v) = trimmed.strip_prefix("description:") {
                return v.trim().trim_matches('"').to_string();
            }
        }
    }
    if let Some(heading) = text.lines().find(|l| l.trim_start().starts_with('#')) {
        return heading.trim_start_matches('#').trim().chars().take(120).collect();
    }
    text.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with("---"))
        .map(|l| l.chars().take(120).collect())
        .unwrap_or_else(|| "no description".into())
}

fn skill_manage(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let name = str_req(args, "name")?;
    if name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("invalid skill name".into());
    }
    let content = str_req(args, "content")?;
    let base = ctx.root.join(DIR).join("skills");
    let canonical_root = ctx.root.canonicalize().map_err(|e| e.to_string())?;
    let mut ancestor = base.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or("invalid skills folder")?;
    }
    if !ancestor.canonicalize().map_err(|e| e.to_string())?.starts_with(&canonical_root) {
        return Err("skills folder escapes the workspace".into());
    }
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let base = base.canonicalize().map_err(|e| e.to_string())?;
    if !base.starts_with(&canonical_root) { return Err("skills folder escapes the workspace".into()); }
    let dir = base.join(&name);
    if dir.is_symlink() { return Err("skill folder cannot be a symlink".into()); }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("SKILL.md");
    if path.exists() && !bool_arg(args, "overwrite") { return Err("skill exists; use overwrite=true to replace it".into()); }
    crate::config::write_private(&path, &format!("{}\n", content.trim_end())).map_err(|e| e.to_string())?;
    Ok(format!("saved workspace skill {name}"))
}

fn skills_list(ctx: &mut Ctx, _args: &Value) -> Result<String, String> {
    let skills = list_skills(ctx);
    if skills.is_empty() {
        return Ok(format!(
            "no skills installed. put them in {}/skills/<name>/SKILL.md",
            DIR
        ));
    }
    let mut out = String::new();
    for (name, desc, _) in skills {
        out.push_str(&format!("{name} · {desc}\n"));
    }
    Ok(out.trim_end().to_string())
}

fn skill_view(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let want = str_req(args, "name")?;
    let skills = list_skills(ctx);
    let Some((_, _, path)) = skills
        .iter()
        .find(|(name, _, _)| name.eq_ignore_ascii_case(&want))
        .cloned()
    else {
        let names: Vec<String> = skills.into_iter().map(|(n, _, _)| n).collect();
        return Err(format!("no skill '{want}'. installed: {}", names.join(", ")));
    };
    let path = if let Some(file) = str_arg(args, "file_path").filter(|s| !s.is_empty()) {
        let base = path.parent().ok_or("skill has no folder")?.canonicalize().map_err(|e| e.to_string())?;
        let target = base.join(file).canonicalize().map_err(|e| e.to_string())?;
        if !target.starts_with(&base) {
            return Err("supporting files must stay inside the skill folder".into());
        }
        target
    } else { path };
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    Ok(super::clip(text))
}

/// The skills line that goes into the system prompt.
pub fn catalog(ctx: &Ctx) -> String {
    let skills = list_skills(ctx);
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from("Skills available (read one with skill_view before using it):\n");
    for (name, desc, _) in skills {
        out.push_str(&format!("- {name}: {desc}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http;
    use crate::tools::testutil::temp_dir;

    fn ctx_in(dir: &Path) -> Ctx {
        Ctx::new(dir.to_path_buf(), http::Cancel::new(), "test".into())
    }

    #[test]
    fn notes_append_and_read_back() {
        let dir = temp_dir("mem");
        let mut ctx = ctx_in(&dir);
        memory_write(&mut ctx, &json!({"content": "the build needs JDK 17"})).unwrap();
        memory_write(&mut ctx, &json!({"content": "tests live in app/src/test"})).unwrap();
        let out = memory_read(&mut ctx, &json!({})).unwrap();
        assert!(out.contains("JDK 17"));
        assert!(out.contains("app/src/test"));
        assert!(dir.join(NOTES).exists());
    }

    #[test]
    fn topics_are_separate_files_and_searchable() {
        let dir = temp_dir("mem-topic");
        let mut ctx = ctx_in(&dir);
        memory_write(&mut ctx, &json!({"content": "uses ratatui 0.30", "topic": "Rust UI"})).unwrap();
        assert!(dir.join(".harness/memory/rustui.md").exists(), "topic names are sanitized");
        let out = memory_search(&mut ctx, &json!({"query": "ratatui"})).unwrap();
        assert!(out.contains("rustui.md"));
        let single = memory_read(&mut ctx, &json!({"topic": "rustui"})).unwrap();
        assert!(single.contains("ratatui 0.30"));
    }

    #[test]
    fn todos_round_trip() {
        let dir = temp_dir("todos");
        let mut ctx = ctx_in(&dir);
        todo_write(
            &mut ctx,
            &json!({"todos": [
                {"content": "port the tools", "status": "done"},
                {"content": "wire the ui", "status": "in_progress"}
            ]}),
        )
        .unwrap();
        let todos = read_todos(&dir);
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].1, "done");
        assert_eq!(todos[1].0, "wire the ui");
    }

    #[test]
    fn broken_todo_files_do_not_crash_the_prompt() {
        let dir = temp_dir("todos-bad");
        fs::create_dir_all(dir.join(".harness")).unwrap();
        fs::write(dir.join(".harness/todos.json"), "{ not json").unwrap();
        assert!(read_todos(&dir).is_empty());
    }

    #[test]
    fn skills_are_discovered_and_viewed() {
        let dir = temp_dir("skills");
        let skill_dir = dir.join(".harness/skills/commit-style");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: commit-style\ndescription: how to write commits here\n---\n\n# Rules\n\nNo em-dashes.\n",
        )
        .unwrap();
        let mut ctx = ctx_in(&dir);
        let listed = skills_list(&mut ctx, &json!({})).unwrap();
        assert!(listed.contains("commit-style"), "{listed}");
        assert!(listed.contains("how to write commits here"));
        let body = skill_view(&mut ctx, &json!({"name": "commit-style"})).unwrap();
        assert!(body.contains("No em-dashes"));
        assert!(catalog(&ctx).contains("commit-style"));
    }

    #[test]
    fn unknown_skills_list_what_exists() {
        let dir = temp_dir("skills-missing");
        let mut ctx = ctx_in(&dir);
        let err = skill_view(&mut ctx, &json!({"name": "nope"})).unwrap_err();
        assert!(err.contains("no skill 'nope'"), "{err}");
    }

    #[test]
    fn descriptions_prefer_frontmatter() {
        assert_eq!(describe("---\ndescription: short\n---\nbody"), "short");
        assert_eq!(describe("# Title\nbody"), "Title");
        assert_eq!(describe("first line\n"), "first line");
    }
    #[test]
    fn skill_management_and_support_files() {
        let dir = temp_dir("skill-manage");
        let mut ctx = ctx_in(&dir);
        let args = json!({"name":"example", "content":"# Example\nFollow these rules."});
        skill_manage(&mut ctx, &args).unwrap();
        assert!(skill_manage(&mut ctx, &args).is_err());
        assert!(skill_manage(&mut ctx, &json!({"name":"../escape", "content":"no"})).is_err());
        let refs = dir.join(".harness/skills/example/references");
        fs::create_dir_all(&refs).unwrap();
        fs::write(refs.join("guide.md"), "support instructions").unwrap();
        let text = skill_view(&mut ctx, &json!({"name":"example", "file_path":"references/guide.md"})).unwrap();
        assert_eq!(text, "support instructions");
        fs::write(dir.join("outside.md"), "outside").unwrap();
        assert!(skill_view(&mut ctx, &json!({"name":"example", "file_path":"../../../outside.md"})).is_err());
        #[cfg(unix)] {
            std::os::unix::fs::symlink(dir.join("outside.md"), refs.join("escape.md")).unwrap();
            assert!(skill_view(&mut ctx, &json!({"name":"example", "file_path":"references/escape.md"})).is_err());
        }
    }

    #[test]
    fn core_read_lists_available_topics() {
        let dir = temp_dir("topic-index");
        let mut ctx = ctx_in(&dir);
        memory_write(&mut ctx, &json!({"topic":"build", "content":"cargo test"})).unwrap();
        assert!(memory_read(&mut ctx, &json!({})).unwrap().contains("build"));
    }

}
