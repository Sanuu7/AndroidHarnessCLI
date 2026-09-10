//! Tools the agent can call.
//!
//! Every tool is a plain function over a JSON argument object. The workspace
//! is a sandbox: paths are resolved against the root and anything that tries
//! to escape it is refused, unless full access is on.

pub mod fs;
pub mod git;
pub mod mem;
pub mod misc;
pub mod net;
pub mod shell;

use crate::llm::{ToolSchema, http};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// How much of a tool result reaches the model. Bigger than a screenful on
/// purpose: reads should not be lossy, but a `cat` of a 2 MB log should be.
pub const MAX_OUTPUT: usize = 24_000;

pub struct Ctx {
    pub root: PathBuf,
    /// Sandbox off: any path on the device, absolute paths included.
    pub full_access: bool,
    pub cancel: http::Cancel,
    /// Which tool call is running, so background jobs can be tagged.
    pub session: String,
}

impl Ctx {
    pub fn new(root: PathBuf, cancel: http::Cancel, session: String) -> Self {
        Self { root, full_access: false, cancel, session }
    }

    /// Resolve a tool path against the workspace, refusing escapes.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, String> {
        let path = path.trim();
        if path.is_empty() {
            return Ok(self.root.clone());
        }
        let raw = Path::new(path);
        let joined = if raw.is_absolute() && self.full_access {
            raw.to_path_buf()
        } else if raw.is_absolute() {
            // Absolute paths are still fine inside the workspace; outside it
            // is exactly the case the sandbox exists for.
            let stripped = raw.strip_prefix(&self.root).map(|p| p.to_path_buf()).ok();
            match stripped {
                Some(p) => self.root.join(p),
                None => {
                    return Err(format!(
                        "{path} is outside the workspace ({}). Use a relative path, or turn on \
                         full access to work anywhere on the device.",
                        self.root.display()
                    ));
                }
            }
        } else {
            self.root.join(raw)
        };
        let normalized = normalize(&joined);
        if !self.full_access && !normalized.starts_with(&self.root) {
            return Err(format!("{path} escapes the workspace"));
        }
        Ok(normalized)
    }

    /// Path as the model should see it: relative to the root when it is inside.
    pub fn display(&self, path: &Path) -> String {
        match path.strip_prefix(&self.root) {
            Ok(rest) if rest.as_os_str().is_empty() => ".".to_string(),
            Ok(rest) => rest.display().to_string(),
            Err(_) => path.display().to_string(),
        }
    }
}

/// Lexical normalization: `..` is folded without touching the filesystem, so
/// it works for paths that do not exist yet.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub struct Tool {
    pub name: &'static str,
    pub desc: &'static str,
    pub params: Value,
    pub run: fn(&mut Ctx, &Value) -> Result<String, String>,
    /// Read-only tools never change the workspace, so they may run in any order.
    pub read_only: bool,
}

impl Tool {
    pub fn schema(&self) -> ToolSchema {
        ToolSchema::new(self.name, self.desc, self.params.clone())
    }
}

/// Every tool, built once. The registry is a static slice so lookups are
/// cheap and the schema list can be handed to a provider without copying.
pub fn all() -> &'static [Tool] {
    static ALL: std::sync::OnceLock<Vec<Tool>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut out = Vec::new();
        out.extend(fs::tools());
        out.extend(shell::tools());
        out.extend(git::tools());
        out.extend(net::tools());
        out.extend(mem::tools());
        out.extend(misc::tools());
        out
    })
}

pub fn find(name: &str) -> Option<&'static Tool> {
    all().iter().find(|t| t.name == name)
}

pub fn schemas() -> Vec<ToolSchema> {
    all().iter().map(|t| t.schema()).collect()
}

/// Run a tool by name, catching the two things that go wrong most: a name
/// the model invented, and a panic inside a tool (a bad file, a broken pipe).
pub fn run(ctx: &mut Ctx, name: &str, args: &str) -> Result<String, String> {
    let Some(tool) = find(name) else {
        let known: Vec<&str> = all().iter().map(|t| t.name).collect();
        return Err(format!("unknown tool '{name}'. available: {}", known.join(", ")));
    };
    let parsed: Value = if args.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(args).map_err(|e| format!("{name}: arguments are not valid JSON ({e})"))?
    };
    // Tool bugs should cost one turn, not the whole session.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (tool.run)(ctx, &parsed))) {
        Ok(r) => r,
        Err(_) => Err(format!("{name} panicked")),
    }
}

/// Clip a result to the model's budget, keeping the head.
pub fn clip(text: String) -> String {
    if text.len() <= MAX_OUTPUT {
        return text;
    }
    let mut end = MAX_OUTPUT;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = text.len() - end;
    format!("{}\n\n… {dropped} bytes dropped (use offset/limit or a narrower query)", &text[..end])
}

pub fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

pub fn str_req(args: &Value, key: &str) -> Result<String, String> {
    str_arg(args, key).filter(|s| !s.trim().is_empty()).ok_or_else(|| format!("{key} is required"))
}

pub fn int_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

pub fn bool_arg(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

#[cfg(test)]
pub mod testutil {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    /// A fresh directory per test, under the system temp dir.
    pub fn temp_dir(tag: &str) -> PathBuf {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "harness-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx::new(PathBuf::from("/work/ws"), http::Cancel::new(), "t".into())
    }

    #[test]
    fn relative_paths_stay_inside() {
        let c = ctx();
        assert_eq!(c.resolve("src/main.rs").unwrap(), PathBuf::from("/work/ws/src/main.rs"));
        assert_eq!(c.resolve("./a/../b").unwrap(), PathBuf::from("/work/ws/b"));
    }

    #[test]
    fn escapes_are_refused() {
        let c = ctx();
        assert!(c.resolve("../secrets").is_err());
        assert!(c.resolve("a/../../secrets").is_err());
        assert!(c.resolve("/etc/passwd").is_err());
    }

    #[test]
    fn full_access_lifts_the_sandbox() {
        let mut c = ctx();
        c.full_access = true;
        assert_eq!(c.resolve("/etc/passwd").unwrap(), PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn listing_shows_the_registry() {
        let mut names: Vec<&str> = all().iter().map(|t| t.name).collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"shell"));
        assert!(names.contains(&"web_fetch"));
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "tool names must be unique");
    }

    #[test]
    fn unknown_tools_are_reported_with_the_list() {
        let mut c = ctx();
        let err = run(&mut c, "nope", "{}").unwrap_err();
        assert!(err.contains("unknown tool"));
        assert!(err.contains("read_file"));
    }

    #[test]
    fn clip_keeps_the_head_and_counts_the_rest() {
        let long = "x".repeat(MAX_OUTPUT + 500);
        let out = clip(long);
        assert!(out.len() < MAX_OUTPUT + 200);
        assert!(out.contains("500 bytes dropped"));
    }
}
