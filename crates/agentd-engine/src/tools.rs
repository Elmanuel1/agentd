//! Workspace tools — path-jailed fs operations (the `agent_toolset_20260401`
//! equivalents), ported from the tosspaper-extraction jail design.
//!
//! Security invariant: every path is canonicalized against the workspace
//! root before any I/O; `..` traversal and absolute escapes are rejected
//! even for not-yet-existing leaves (ancestor-walk canonicalization).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Serialize;

/// Tool execution outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolOutcome {
    /// Text output for the model.
    pub output: String,
    /// Whether the tool errored (surfaced as `is_error`).
    pub is_error: bool,
}

impl ToolOutcome {
    fn ok(s: impl Into<String>) -> Self {
        Self {
            output: s.into(),
            is_error: false,
        }
    }
    fn err(s: impl Into<String>) -> Self {
        Self {
            output: s.into(),
            is_error: true,
        }
    }
}

/// The tool-execution port.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute one named tool with its JSON input.
    async fn execute(&self, name: &str, input: &serde_json::Value) -> ToolOutcome;
}

/// Path-jailed workspace tools: read / write / edit / glob / grep.
/// (`bash` and `web_*` are intentionally NOT implemented here — bash needs a
/// sandbox boundary the Runtime provides, web tools an egress proxy.)
#[derive(Debug)]
pub struct WorkspaceTools {
    root: PathBuf,
}

impl WorkspaceTools {
    /// Jail all operations under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Canonicalize `relative` against the jail; rejects escapes even when
    /// the leaf does not exist yet (walks up to the deepest existing
    /// ancestor, canonicalizes that, re-attaches the tail).
    fn jail(&self, relative: &str) -> Result<PathBuf, String> {
        if Path::new(relative).is_absolute() {
            return Err(format!("path {relative} is outside the workspace"));
        }
        let resolved = self.root.join(relative);
        let canonical_root = self
            .root
            .canonicalize()
            .map_err(|e| format!("workspace unavailable: {e}"))?;
        let canonical = if resolved.exists() {
            resolved
                .canonicalize()
                .map_err(|e| format!("cannot resolve {relative}: {e}"))?
        } else {
            let mut existing = resolved.as_path();
            let mut tail: Vec<std::ffi::OsString> = Vec::new();
            loop {
                if existing.exists() {
                    break;
                }
                match (existing.file_name(), existing.parent()) {
                    (Some(name), Some(parent)) => {
                        tail.push(name.to_owned());
                        existing = parent;
                    }
                    _ => return Err(format!("path {relative} is outside the workspace")),
                }
            }
            let mut c = existing
                .canonicalize()
                .map_err(|e| format!("cannot resolve {relative}: {e}"))?;
            for seg in tail.iter().rev() {
                c.push(seg);
            }
            c
        };
        if !canonical.starts_with(&canonical_root) {
            return Err(format!("path {relative} is outside the workspace"));
        }
        Ok(canonical)
    }

    fn read(&self, input: &serde_json::Value) -> ToolOutcome {
        let Some(path) = input["path"].as_str() else {
            return ToolOutcome::err("read: `path` is required");
        };
        match self.jail(path).and_then(|p| {
            std::fs::read_to_string(&p).map_err(|e| format!("read {path}: {e}"))
        }) {
            Ok(content) => ToolOutcome::ok(content),
            Err(e) => ToolOutcome::err(e),
        }
    }

    fn write(&self, input: &serde_json::Value) -> ToolOutcome {
        let (Some(path), Some(content)) = (input["path"].as_str(), input["content"].as_str())
        else {
            return ToolOutcome::err("write: `path` and `content` are required");
        };
        match self.jail(path) {
            Ok(p) => {
                if let Some(parent) = p.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return ToolOutcome::err(format!("write {path}: {e}"));
                    }
                }
                match std::fs::write(&p, content) {
                    Ok(()) => ToolOutcome::ok(format!("wrote {} bytes to {path}", content.len())),
                    Err(e) => ToolOutcome::err(format!("write {path}: {e}")),
                }
            }
            Err(e) => ToolOutcome::err(e),
        }
    }

    fn edit(&self, input: &serde_json::Value) -> ToolOutcome {
        let (Some(path), Some(old), Some(new)) = (
            input["path"].as_str(),
            input["old_str"].as_str(),
            input["new_str"].as_str(),
        ) else {
            return ToolOutcome::err("edit: `path`, `old_str`, `new_str` are required");
        };
        let p = match self.jail(path) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e),
        };
        let content = match std::fs::read_to_string(&p) {
            Ok(c) => c,
            Err(e) => return ToolOutcome::err(format!("edit {path}: {e}")),
        };
        let matches = content.matches(old).count();
        if matches == 0 {
            return ToolOutcome::err(format!("edit {path}: old_str not found"));
        }
        if matches > 1 {
            return ToolOutcome::err(format!(
                "edit {path}: old_str matches {matches} times; must be unique"
            ));
        }
        let updated = content.replacen(old, new, 1);
        match std::fs::write(&p, updated) {
            Ok(()) => ToolOutcome::ok(format!("edited {path}")),
            Err(e) => ToolOutcome::err(format!("edit {path}: {e}")),
        }
    }

    fn glob(&self, input: &serde_json::Value) -> ToolOutcome {
        let Some(pattern) = input["pattern"].as_str() else {
            return ToolOutcome::err("glob: `pattern` is required");
        };
        if pattern.contains("..") || Path::new(pattern).is_absolute() {
            return ToolOutcome::err(format!("glob: pattern {pattern} escapes the workspace"));
        }
        let full = self.root.join(pattern);
        let Some(full_str) = full.to_str() else {
            return ToolOutcome::err("glob: non-utf8 pattern");
        };
        match glob::glob(full_str) {
            Ok(paths) => {
                let mut out: Vec<String> = paths
                    .filter_map(Result::ok)
                    .filter_map(|p| {
                        p.strip_prefix(&self.root)
                            .ok()
                            .map(|r| r.to_string_lossy().into_owned())
                    })
                    .collect();
                out.sort();
                ToolOutcome::ok(out.join("\n"))
            }
            Err(e) => ToolOutcome::err(format!("glob: {e}")),
        }
    }

    fn grep(&self, input: &serde_json::Value) -> ToolOutcome {
        let Some(pattern) = input["pattern"].as_str() else {
            return ToolOutcome::err("grep: `pattern` is required");
        };
        let path = input["path"].as_str().unwrap_or(".");
        let root = match self.jail(path) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::err(e),
        };
        let mut hits = Vec::new();
        grep_walk(&root, pattern, &self.root, &mut hits);
        hits.sort();
        ToolOutcome::ok(hits.join("\n"))
    }
}

fn grep_walk(dir: &Path, needle: &str, root: &Path, hits: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            // Single file case.
            if let Ok(content) = std::fs::read_to_string(dir) {
                collect_hits(dir, &content, needle, root, hits);
            }
            return;
        }
    };
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            grep_walk(&p, needle, root, hits);
        } else if let Ok(content) = std::fs::read_to_string(&p) {
            collect_hits(&p, &content, needle, root, hits);
        }
    }
}

fn collect_hits(path: &Path, content: &str, needle: &str, root: &Path, hits: &mut Vec<String>) {
    let rel = path
        .strip_prefix(root)
        .map(|r| r.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    for (i, line) in content.lines().enumerate() {
        if line.contains(needle) {
            hits.push(format!("{rel}:{}:{line}", i + 1));
        }
    }
}

#[async_trait]
impl ToolExecutor for WorkspaceTools {
    async fn execute(&self, name: &str, input: &serde_json::Value) -> ToolOutcome {
        match name {
            "read" => self.read(input),
            "write" => self.write(input),
            "edit" => self.edit(input),
            "glob" => self.glob(input),
            "grep" => self.grep(input),
            other => ToolOutcome::err(format!("unknown tool `{other}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rig() -> (tempfile::TempDir, WorkspaceTools) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "hello agentd\nline two\n").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.md"), "hello nested\n").unwrap();
        let tools = WorkspaceTools::new(dir.path());
        (dir, tools)
    }

    #[tokio::test]
    async fn read_write_edit_roundtrip() {
        let (_d, t) = rig();
        let r = t
            .execute("read", &serde_json::json!({"path":"a.md"}))
            .await;
        assert!(!r.is_error);
        assert!(r.output.contains("hello agentd"));

        let w = t
            .execute(
                "write",
                &serde_json::json!({"path":"new/dir/c.txt","content":"created"}),
            )
            .await;
        assert!(!w.is_error, "{}", w.output);

        let e = t
            .execute(
                "edit",
                &serde_json::json!({"path":"a.md","old_str":"line two","new_str":"line 2"}),
            )
            .await;
        assert!(!e.is_error);
        let r = t
            .execute("read", &serde_json::json!({"path":"a.md"}))
            .await;
        assert!(r.output.contains("line 2"));
    }

    #[tokio::test]
    async fn edit_requires_unique_match() {
        let (_d, t) = rig();
        t.execute("write", &serde_json::json!({"path":"dup.txt","content":"x x"}))
            .await;
        let e = t
            .execute(
                "edit",
                &serde_json::json!({"path":"dup.txt","old_str":"x","new_str":"y"}),
            )
            .await;
        assert!(e.is_error);
        assert!(e.output.contains("2 times"));
    }

    #[tokio::test]
    async fn traversal_and_absolute_paths_are_jailed() {
        let (_d, t) = rig();
        for path in ["../etc/passwd", "sub/../../escape.txt", "/etc/passwd"] {
            let r = t.execute("read", &serde_json::json!({ "path": path })).await;
            assert!(r.is_error, "{path} must be rejected");
            let w = t
                .execute("write", &serde_json::json!({"path": path, "content":"x"}))
                .await;
            assert!(w.is_error, "{path} write must be rejected");
        }
    }

    #[tokio::test]
    async fn glob_and_grep_stay_inside() {
        let (_d, t) = rig();
        let g = t
            .execute("glob", &serde_json::json!({"pattern":"**/*.md"}))
            .await;
        assert!(!g.is_error);
        assert!(g.output.contains("a.md"));
        assert!(g.output.contains("sub/b.md"));
        // Escaping glob rejected.
        let bad = t
            .execute("glob", &serde_json::json!({"pattern":"../**/*"}))
            .await;
        assert!(bad.is_error);

        let hits = t
            .execute("grep", &serde_json::json!({"pattern":"hello"}))
            .await;
        assert!(hits.output.contains("a.md:1:hello agentd"));
        assert!(hits.output.contains("sub/b.md:1:hello nested"));
    }

    #[tokio::test]
    async fn unknown_tool_errors() {
        let (_d, t) = rig();
        let r = t.execute("bash", &serde_json::json!({"command":"rm -rf /"})).await;
        assert!(r.is_error);
        assert!(r.output.contains("unknown tool"));
    }
}
