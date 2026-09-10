//! Files: read, write, edit, list, search.
//!
//! The two rules that matter in here: edits fail loudly instead of guessing
//! when the text they were given is not in the file, and a read that would
//! flood the conversation says so instead of flooding it.

use super::{Ctx, Tool, bool_arg, int_arg, str_arg, str_req};
use crate::llm::{bool_prop, number_prop, schema, string_prop};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// The file tools. Built on demand: JSON schemas cannot be const.
pub fn tools() -> Vec<Tool> {
    vec![
    Tool {
        name: "read_file",
        desc: "Read a text file. Returns numbered lines so you can quote them exactly.",
        params: schema(
            json!({
                "path": string_prop("File path, relative to the workspace"),
                "offset": number_prop("First line to read, 1-based"),
                "limit": number_prop("How many lines to read, default 400"),
            }),
            &["path"],
        ),
        run: read_file,
        read_only: true,
    },
    Tool {
        name: "write_file",
        desc: "Create or overwrite a file with the given content.",
        params: schema(
            json!({
                "path": string_prop("File path"),
                "content": string_prop("Full file content"),
            }),
            &["path", "content"],
        ),
        run: write_file,
        read_only: false,
    },
    Tool {
        name: "edit_file",
        desc: "Replace an exact block of text in a file. Fails if the old text is \
               missing or ambiguous.",
        params: schema(
            json!({
                "path": string_prop("File path"),
                "old": string_prop("Text to replace, copied exactly from the file"),
                "new": string_prop("Replacement text"),
                "replace_all": bool_prop("Replace every occurrence, default false"),
            }),
            &["path", "old", "new"],
        ),
        run: edit_file,
        read_only: false,
    },
    Tool {
        name: "multi_edit",
        desc: "Apply several edits to one file in order. All must match.",
        params: schema(
            json!({
                "path": string_prop("File path"),
                "edits": json!({
                    "type": "array",
                    "description": "Edits, applied in order",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old": string_prop("Text to replace"),
                            "new": string_prop("Replacement"),
                        },
                        "required": ["old", "new"],
                    },
                }),
            }),
            &["path", "edits"],
        ),
        run: multi_edit,
        read_only: false,
    },
    Tool {
        name: "list_dir",
        desc: "List a directory with sizes, directories first.",
        params: schema(
            json!({
                "path": string_prop("Directory, default the workspace root"),
            }),
            &[],
        ),
        run: list_dir,
        read_only: true,
    },
    Tool {
        name: "search_files",
        desc: "Find files by glob pattern, e.g. '**/*.kt' or 'src/*.rs'.",
        params: schema(
            json!({
                "pattern": string_prop("Glob pattern"),
                "path": string_prop("Directory to search from, default the root"),
                "max": number_prop("Maximum results, default 80"),
            }),
            &["pattern"],
        ),
        run: search_files,
        read_only: true,
    },
    Tool {
        name: "grep",
        desc: "Search file contents. Literal text by default, set regex for a pattern.",
        params: schema(
            json!({
                "pattern": string_prop("Text to find, or an extended regex when regex is true"),
                "path": string_prop("File or directory to search, default the root"),
                "glob": string_prop("Only search files matching this glob"),
                "ignore_case": bool_prop("Case insensitive"),
                "regex": bool_prop("Treat the pattern as an extended regex"),
                "max": number_prop("Maximum matching lines, default 60"),
            }),
            &["pattern"],
        ),
        run: grep,
        read_only: true,
    },
    Tool {
        name: "file_info",
        desc: "Size, line count, and type of a path.",
        params: schema(json!({ "path": string_prop("Path to inspect") }), &["path"]),
        run: file_info,
        read_only: true,
    },
    Tool {
        name: "create_dir",
        desc: "Create a directory, parents included.",
        params: schema(json!({ "path": string_prop("Directory to create") }), &["path"]),
        run: create_dir,
        read_only: false,
    },
    Tool {
        name: "move_file",
        desc: "Move or rename a file or directory.",
        params: schema(
            json!({
                "from": string_prop("Source path"),
                "to": string_prop("Destination path"),
            }),
            &["from", "to"],
        ),
        run: move_file,
        read_only: false,
    },
    Tool {
        name: "delete_file",
        desc: "Delete a file, or a directory when recursive is true.",
        params: schema(
            json!({
                "path": string_prop("Path to delete"),
                "recursive": bool_prop("Allow deleting a non-empty directory"),
            }),
            &["path"],
        ),
        run: delete_file,
        read_only: false,
    },
    ]
}

fn read_file(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let path = ctx.resolve(&str_req(args, "path")?)?;
    let offset = int_arg(args, "offset").unwrap_or(1).max(1) as usize;
    let limit = int_arg(args, "limit").unwrap_or(400).clamp(1, 5_000) as usize;
    let file = fs::File::open(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    if is_binary(&path) {
        return Err(format!(
            "{} looks binary ({} bytes); read_image or shell handles those",
            ctx.display(&path),
            fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        ));
    }
    let mut out = String::new();
    let mut total = 0usize;
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        total += 1;
        if i + 1 < offset {
            continue;
        }
        if i + 1 >= offset + limit {
            continue;
        }
        out.push_str(&format!("{:>5}│ {}\n", i + 1, line));
    }
    if out.is_empty() {
        return Ok(format!("{} is empty", ctx.display(&path)));
    }
    if total > offset + limit - 1 {
        out.push_str(&format!("… {} more lines\n", total - (offset + limit - 1)));
    }
    Ok(super::clip(out))
}

fn write_file(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let path = ctx.resolve(&str_req(args, "path")?)?;
    let content = str_arg(args, "content").unwrap_or_default();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", ctx.display(dir)))?;
    }
    let existed = fs::read_to_string(&path).ok();
    fs::write(&path, &content).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    let name = ctx.display(&path);
    match existed {
        // Overwriting is an edit, and an edit should show what it did.
        Some(before) if before != content => Ok(edit_report(&name, &before, &content, 1)),
        Some(_) => Ok(format!("wrote {name} (unchanged)")),
        None => Ok(format!(
            "created {name} ({} bytes, {} lines)",
            content.len(),
            content.lines().count()
        )),
    }
}

/// Apply one text replacement, exact first and line-trimmed as a fallback.
pub fn replace_block(source: &str, old: &str, new: &str, all: bool) -> Result<(String, usize), String> {
    if old.is_empty() {
        return Err("old text is empty".into());
    }
    let count = source.matches(old).count();
    if count == 1 || (count > 1 && all) {
        let out = if all {
            source.replace(old, new)
        } else {
            source.replacen(old, new, 1)
        };
        return Ok((out, if all { count } else { 1 }));
    }
    if count > 1 {
        return Err(format!("old text appears {count} times; add more context or set replace_all"));
    }
    // Not an exact match: try again ignoring indentation, which is the usual
    // reason a copy from the screen does not line up.
    let source_lines: Vec<&str> = source.lines().collect();
    let old_lines: Vec<&str> = old.lines().filter(|l| !l.trim().is_empty()).collect();
    if old_lines.is_empty() {
        return Err("old text is empty".into());
    }
    let mut hits = 0usize;
    let mut hit_at = 0usize;
    for start in 0..source_lines.len() {
        if start + old_lines.len() > source_lines.len() {
            break;
        }
        let matched = old_lines.iter().enumerate().all(|(i, want)| {
            source_lines[start + i].trim() == want.trim()
        });
        if matched {
            hits += 1;
            hit_at = start;
        }
    }
    if hits == 0 {
        return Err("old text not found (read the file again and copy it exactly)".into());
    }
    if hits > 1 && !all {
        return Err(format!("old text is ambiguous: {hits} places match once indentation is ignored"));
    }
    let mut out_lines: Vec<String> = source_lines.iter().map(|l| l.to_string()).collect();
    let new_lines: Vec<String> = new.lines().map(|l| l.to_string()).collect();
    out_lines.splice(hit_at..hit_at + old_lines.len(), new_lines);
    let mut out = out_lines.join("\n");
    if source.ends_with('\n') {
        out.push('\n');
    }
    let _ = hit_at;
    Ok((out, 1))
}

fn edit_file(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let path = ctx.resolve(&str_req(args, "path")?)?;
    let old = str_req(args, "old")?;
    let new = str_arg(args, "new").unwrap_or_default();
    let all = bool_arg(args, "replace_all");
    let source = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    let (updated, n) = replace_block(&source, &old, &new, all)?;
    fs::write(&path, &updated).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    Ok(edit_report(&ctx.display(&path), &source, &updated, n))
}

fn multi_edit(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let path = ctx.resolve(&str_req(args, "path")?)?;
    let edits = args
        .get("edits")
        .and_then(|e| e.as_array())
        .ok_or("edits must be an array")?;
    let source = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    let mut updated = source.clone();
    let mut applied = 0usize;
    for (i, edit) in edits.iter().enumerate() {
        let old = str_req(edit, "old").map_err(|e| format!("edit {}: {e}", i + 1))?;
        let new = str_arg(edit, "new").unwrap_or_default();
        let (next, n) = replace_block(&updated, &old, &new, false)
            .map_err(|e| format!("edit {}: {e}", i + 1))?;
        updated = next;
        applied += n;
    }
    fs::write(&path, &updated).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    Ok(edit_report(&ctx.display(&path), &source, &updated, applied))
}

/// Every write is reported as what changed, not just that something did.
/// Counts lead the line so a truncated preview still says how big the change
/// was, and the diff lines carry their numbers for the transcript to colour.
fn edit_report(path: &str, before: &str, after: &str, replacements: usize) -> String {
    let lines = crate::diff::unified(before, after, 2);
    let (added, removed) = crate::diff::counts(&lines);
    if added == 0 && removed == 0 {
        return format!("+0 -0  {path} ({replacements} replacements, no net change)");
    }
    let mut out = format!("+{added} -{removed}  edited {path}\n");
    for line in &lines {
        out.push_str(&line.to_text());
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn list_dir(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let path = ctx.resolve(&str_arg(args, "path").unwrap_or_else(|| ".".into()))?;
    let mut dirs: Vec<(String, u64)> = Vec::new();
    let mut files: Vec<(String, u64)> = Vec::new();
    let entries = fs::read_dir(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    let mut count = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" || name == "node_modules" || name == "target" {
            continue;
        }
        count += 1;
        if count > 400 {
            break;
        }
        let meta = entry.metadata().ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        if meta.as_ref().is_some_and(|m| m.is_dir()) {
            dirs.push((format!("{name}/"), 0));
        } else {
            files.push((name, size));
        }
    }
    dirs.sort();
    files.sort();
    let mut out = format!("{}\n", ctx.display(&path));
    for (name, _) in &dirs {
        out.push_str(&format!("  {name}\n"));
    }
    for (name, size) in &files {
        out.push_str(&format!("  {name}  {}\n", human_size(*size)));
    }
    if dirs.is_empty() && files.is_empty() {
        out.push_str("  (empty)\n");
    }
    Ok(super::clip(out))
}

fn search_files(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let pattern = str_req(args, "pattern")?;
    let start = ctx.resolve(&str_arg(args, "path").unwrap_or_else(|| ".".into()))?;
    let max = int_arg(args, "max").unwrap_or(80).clamp(1, 500) as usize;
    let mut hits: Vec<String> = Vec::new();
    walk(&start, 12, &mut |path| {
        if hits.len() >= max {
            return;
        }
        let rel = ctx.display(path);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let ok = glob_match(&pattern, &rel) || glob_match(&pattern, &name);
        if ok {
            hits.push(rel);
        }
    });
    if hits.is_empty() {
        return Ok(format!("no files match {pattern}"));
    }
    let mut out = hits.join("\n");
    if hits.len() >= max {
        out.push_str(&format!("\n… capped at {max} results"));
    }
    Ok(out)
}

fn grep(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let pattern = str_req(args, "pattern")?;
    let target = ctx.resolve(&str_arg(args, "path").unwrap_or_else(|| ".".into()))?;
    let glob = str_arg(args, "glob");
    let ignore_case = bool_arg(args, "ignore_case");
    let max = int_arg(args, "max").unwrap_or(60).clamp(1, 400) as usize;
    let needle = if ignore_case { pattern.to_lowercase() } else { pattern.clone() };
    let regex = if bool_arg(args, "regex") { Some(pattern.clone()) } else { None };

    if bool_arg(args, "regex") {
        // An extended regex without pulling in a regex engine: ask grep.
        return grep_via_shell(ctx, &target, &pattern, glob.as_deref(), ignore_case, max);
    }

    let mut out = String::new();
    let mut hits = 0usize;
    let mut scan = |path: &Path| {
        if hits >= max {
            return;
        }
        if let Some(g) = &glob {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !glob_match(g, &name) && !glob_match(g, &ctx.display(path)) {
                return;
            }
        }
        if is_binary(path) {
            return;
        }
        let Ok(file) = fs::File::open(path) else { return };
        for (i, line) in BufReader::new(file).lines().take(20_000).enumerate() {
            let Ok(line) = line else { break };
            let hay = if ignore_case { line.to_lowercase() } else { line.clone() };
            if hay.contains(&needle) {
                hits += 1;
                out.push_str(&format!("{}:{}: {}\n", ctx.display(path), i + 1, trim_line(&line, 240)));
                if hits >= max {
                    break;
                }
            }
        }
    };
    if target.is_file() {
        scan(&target);
    } else {
        walk(&target, 14, &mut scan);
    }
    if out.is_empty() {
        return Ok(format!("no matches for {}", regex.unwrap_or(needle)));
    }
    if hits >= max {
        out.push_str(&format!("… capped at {max} matches\n"));
    }
    Ok(super::clip(out))
}

fn grep_via_shell(
    ctx: &mut Ctx,
    target: &Path,
    pattern: &str,
    glob: Option<&str>,
    ignore_case: bool,
    max: usize,
) -> Result<String, String> {
    let mut cmd = format!(
        "grep -rIn {} --exclude-dir=.git --exclude-dir=node_modules --exclude-dir=target",
        if ignore_case { "-i" } else { "" }
    );
    if let Some(g) = glob {
        cmd.push_str(&format!(" --include={}", shell_quote(g)));
    }
    cmd.push_str(&format!(" -E {} {}", shell_quote(pattern), shell_quote(&target.display().to_string())));
    let out = super::shell::run_capture(ctx, &cmd, 60_000)?;
    if out.trim().is_empty() {
        return Ok(format!("no matches for {pattern}"));
    }
    let mut lines: Vec<&str> = out.lines().take(max).collect();
    let capped = out.lines().count() > max;
    let mut text = lines.join("\n");
    if capped {
        text.push_str(&format!("\n… capped at {max} matches"));
    }
    let _ = lines.pop();
    Ok(super::clip(text))
}

fn file_info(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let path = ctx.resolve(&str_req(args, "path")?)?;
    let meta = fs::metadata(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    if meta.is_dir() {
        let count = fs::read_dir(&path).map(|d| d.count()).unwrap_or(0);
        return Ok(format!("{} · directory · {count} entries", ctx.display(&path)));
    }
    let lines = fs::read_to_string(&path).map(|s| s.lines().count()).unwrap_or(0);
    Ok(format!(
        "{} · {} bytes · {} lines",
        ctx.display(&path),
        meta.len(),
        lines
    ))
}

fn create_dir(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let path = ctx.resolve(&str_req(args, "path")?)?;
    fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    Ok(format!("created {}", ctx.display(&path)))
}

fn move_file(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let from = ctx.resolve(&str_req(args, "from")?)?;
    let to = ctx.resolve(&str_req(args, "to")?)?;
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", ctx.display(dir)))?;
    }
    fs::rename(&from, &to).map_err(|e| format!("rename failed: {e}"))?;
    Ok(format!("moved {} → {}", ctx.display(&from), ctx.display(&to)))
}

fn delete_file(ctx: &mut Ctx, args: &Value) -> Result<String, String> {
    let path = ctx.resolve(&str_req(args, "path")?)?;
    let meta = fs::metadata(&path).map_err(|e| format!("{}: {e}", ctx.display(&path)))?;
    if meta.is_dir() {
        if !bool_arg(args, "recursive") {
            return Err(format!("{} is a directory; set recursive to delete it", ctx.display(&path)));
        }
        fs::remove_dir_all(&path).map_err(|e| e.to_string())?;
    } else {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(format!("deleted {}", ctx.display(&path)))
}

// -- helpers ---------------------------------------------------------------

pub fn human_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes}B"),
        1024..=1_048_575 => format!("{:.1}K", bytes as f32 / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1}M", bytes as f32 / 1_048_576.0),
        _ => format!("{:.1}G", bytes as f32 / 1_073_741_824.0),
    }
}

fn trim_line(line: &str, max: usize) -> String {
    if line.chars().count() <= max {
        return line.to_string();
    }
    line.chars().take(max).collect::<String>() + "…"
}

/// A NUL byte in the first 512 means do not treat this as text.
pub fn is_binary(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else { return false };
    let mut buf = [0u8; 512];
    let read = std::io::Read::read(&mut file, &mut buf).unwrap_or(0);
    buf[..read].contains(&0)
}

/// Directories that are never worth walking.
fn noisy(name: &str) -> bool {
    matches!(name, ".git" | "node_modules" | "target" | "build" | ".gradle" | "dist") || name == ".harness"
}

pub fn walk(root: &Path, depth: usize, f: &mut impl FnMut(&Path)) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') && name != ".harness" {
            continue;
        }
        if noisy(&name) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(&path, depth - 1, f);
        } else {
            f(&path);
        }
    }
}

/// Glob matching with `*`, `?`, and `**`, segment aware.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let parts: Vec<&str> = path.split('/').collect();
    match_segments(&pat, &parts)
}

fn match_segments(pat: &[&str], parts: &[&str]) -> bool {
    if pat.is_empty() {
        return parts.is_empty();
    }
    if pat[0] == "**" {
        for skip in 0..=parts.len() {
            if match_segments(&pat[1..], &parts[skip..]) {
                return true;
            }
        }
        return false;
    }
    if parts.is_empty() {
        return false;
    }
    if !segment_match(pat[0], parts[0]) {
        return false;
    }
    match_segments(&pat[1..], &parts[1..])
}

fn segment_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    segment_chars(&p, &t)
}

fn segment_chars(pat: &[char], text: &[char]) -> bool {
    match pat.first() {
        None => text.is_empty(),
        Some('*') => (0..=text.len()).any(|skip| segment_chars(&pat[1..], &text[skip..])),
        Some('?') => !text.is_empty() && segment_chars(&pat[1..], &text[1..]),
        Some(c) => text.first() == Some(c) && segment_chars(&pat[1..], &text[1..]),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
    fn reads_with_line_numbers_and_windows() {
        let dir = temp_dir("read");
        fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let mut ctx = ctx_in(&dir);
        let out = read_file(&mut ctx, &json!({"path": "a.txt", "offset": 2, "limit": 5})).unwrap();
        assert!(out.contains("    2│ two"));
        assert!(out.contains("    3│ three"));
        assert!(!out.contains("one"));
    }

    #[test]
    fn write_then_edit_round_trip() {
        let dir = temp_dir("edit");
        let mut ctx = ctx_in(&dir);
        write_file(&mut ctx, &json!({"path": "src/x.rs", "content": "fn main() {}\n"})).unwrap();
        assert!(dir.join("src/x.rs").exists());
        let out = edit_file(
            &mut ctx,
            &json!({"path": "src/x.rs", "old": "fn main() {}", "new": "fn main() { run() }"}),
        )
        .unwrap();
        assert!(out.starts_with("+1 -1"), "{out}");
        assert!(out.contains("-1 fn main() {}"), "the diff shows both sides: {out}");
        assert!(out.contains("+1 fn main() { run() }"), "{out}");
        let text = fs::read_to_string(dir.join("src/x.rs")).unwrap();
        assert_eq!(text, "fn main() { run() }\n");
    }

    #[test]
    fn edits_fail_loudly_when_text_is_missing() {
        let dir = temp_dir("missing");
        fs::write(dir.join("a.txt"), "hello\n").unwrap();
        let mut ctx = ctx_in(&dir);
        let err = edit_file(&mut ctx, &json!({"path": "a.txt", "old": "nope", "new": "x"})).unwrap_err();
        assert!(err.contains("not found"), "{err}");
        // And nothing was written.
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "hello\n");
    }

    #[test]
    fn ambiguous_edits_are_refused_unless_replace_all() {
        let dir = temp_dir("ambiguous");
        fs::write(dir.join("a.txt"), "x\nx\n").unwrap();
        let mut ctx = ctx_in(&dir);
        let err = edit_file(&mut ctx, &json!({"path": "a.txt", "old": "x", "new": "y"})).unwrap_err();
        assert!(err.contains("2 times"), "{err}");
        edit_file(&mut ctx, &json!({"path": "a.txt", "old": "x", "new": "y", "replace_all": true}))
            .unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "y\ny\n");
    }

    #[test]
    fn indentation_only_mismatch_still_edits() {
        let dir = temp_dir("indent");
        fs::write(dir.join("a.py"), "def f():\n    return 1\n").unwrap();
        let mut ctx = ctx_in(&dir);
        edit_file(&mut ctx, &json!({"path": "a.py", "old": "def f():\nreturn 1", "new": "def f():\n    return 2"}))
            .unwrap();
        assert!(fs::read_to_string(dir.join("a.py")).unwrap().contains("return 2"));
    }

    #[test]
    fn multi_edit_applies_in_order() {
        let dir = temp_dir("multi");
        fs::write(dir.join("a.txt"), "a\nb\nc\n").unwrap();
        let mut ctx = ctx_in(&dir);
        multi_edit(
            &mut ctx,
            &json!({"path": "a.txt", "edits": [{"old": "a", "new": "A"}, {"old": "c", "new": "C"}]}),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "A\nb\nC\n");
    }

    #[test]
    fn list_dir_sorts_dirs_first_and_skips_noise() {
        let dir = temp_dir("list");
        fs::create_dir_all(dir.join("zeta")).unwrap();
        fs::create_dir_all(dir.join("node_modules")).unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();
        let mut ctx = ctx_in(&dir);
        let out = list_dir(&mut ctx, &json!({})).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[1].contains("zeta/"), "{out}");
        assert!(!out.contains("node_modules"));
        assert!(out.contains("b.txt"));
    }

    #[test]
    fn search_files_matches_globs() {
        let dir = temp_dir("glob");
        fs::create_dir_all(dir.join("src/deep")).unwrap();
        fs::write(dir.join("src/main.rs"), "").unwrap();
        fs::write(dir.join("src/deep/lib.rs"), "").unwrap();
        fs::write(dir.join("src/notes.md"), "").unwrap();
        let mut ctx = ctx_in(&dir);
        let out = search_files(&mut ctx, &json!({"pattern": "**/*.rs"})).unwrap();
        assert!(out.contains("main.rs"));
        assert!(out.contains("lib.rs"));
        assert!(!out.contains("notes.md"));
        let out = search_files(&mut ctx, &json!({"pattern": "src/*.rs"})).unwrap();
        assert!(out.contains("main.rs"));
        assert!(!out.contains("deep"), "single star does not cross directories");
    }

    #[test]
    fn grep_finds_lines_and_respects_case_flag() {
        let dir = temp_dir("grep");
        fs::write(dir.join("a.txt"), "Hello\nworld\n").unwrap();
        let mut ctx = ctx_in(&dir);
        let out = grep(&mut ctx, &json!({"pattern": "hello"})).unwrap();
        assert!(out.contains("no matches"));
        let out = grep(&mut ctx, &json!({"pattern": "hello", "ignore_case": true})).unwrap();
        assert!(out.contains("a.txt:1: Hello"));
    }

    #[test]
    fn grep_skips_binaries() {
        let dir = temp_dir("bin");
        fs::write(dir.join("blob.bin"), b"\x00\x01needle\x02").unwrap();
        let mut ctx = ctx_in(&dir);
        let out = grep(&mut ctx, &json!({"pattern": "needle"})).unwrap();
        assert!(out.contains("no matches"), "{out}");
    }

    #[test]
    fn delete_and_move_behave() {
        let dir = temp_dir("mv");
        fs::write(dir.join("a.txt"), "x").unwrap();
        let mut ctx = ctx_in(&dir);
        move_file(&mut ctx, &json!({"from": "a.txt", "to": "sub/b.txt"})).unwrap();
        assert!(dir.join("sub/b.txt").exists());
        delete_file(&mut ctx, &json!({"path": "sub"})).unwrap_err();
        delete_file(&mut ctx, &json!({"path": "sub", "recursive": true})).unwrap();
        assert!(!dir.join("sub").exists());
    }

    #[test]
    fn glob_patterns() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.py"));
        assert!(glob_match("**/*.rs", "src/deep/lib.rs"));
        assert!(glob_match("src/?ain.rs", "src/main.rs"));
        assert!(!glob_match("src/*.rs", "src/deep/lib.rs"));
    }

    #[test]
    fn sizes_read_like_sizes() {
        assert_eq!(human_size(12), "12B");
        assert_eq!(human_size(2048), "2.0K");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0M");
    }
}
