//! The file tools: `read`, `write`, `edit`, `glob` and `grep`, each a
//! command in the sandbox as the agent's user.

use serde_json::Value;

use super::sandbox::{combined, shell, str_arg, SandboxTools, FILE_TIMEOUT, MAX_MATCHES, WORKDIR};
use super::text;
use super::ToolOutcome;

impl SandboxTools {
    pub(super) async fn cat(&self, path: &str) -> Result<String, ToolOutcome> {
        let out = self
            .run(
                shell(
                    r#"if [ -d "$1" ]; then echo "$1 is a directory" >&2; exit 2; fi; cat -- "$1""#,
                    &[path],
                ),
                None,
                FILE_TIMEOUT,
            )
            .await?;
        if out.exit_code != 0 {
            return Err(ToolOutcome::err(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Write `content` at `path`; whether the file existed before.
    pub(super) async fn put(&self, path: &str, content: &str) -> Result<bool, ToolOutcome> {
        let script = r#"mkdir -p "$(dirname "$1")" || exit 1; if [ -e "$1" ]; then e=1; else e=0; fi; cat > "$1" || exit 1; echo $e"#;
        let out = self.run(shell(script, &[path]), Some(content.as_bytes().to_vec()), FILE_TIMEOUT).await?;
        if out.exit_code != 0 {
            return Err(ToolOutcome::err(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim() == "1")
    }

    pub(super) async fn read(&self, input: &Value) -> Result<ToolOutcome, ToolOutcome> {
        let path = str_arg(input, "file_path", "read")?;
        let range = match crate::json::at(input, "/view_range").as_array().map(Vec::as_slice) {
            Some([s, e]) => Some((s.as_i64().unwrap_or(1), e.as_i64().unwrap_or(-1))),
            Some(_) => return Err(ToolOutcome::err("read: `view_range` is [start, end]")),
            None => None,
        };
        let content = self.cat(path).await?;
        text::numbered(&content, range).map(ToolOutcome::ok).map_err(ToolOutcome::err)
    }

    pub(super) async fn write(&self, input: &Value) -> Result<ToolOutcome, ToolOutcome> {
        let path = str_arg(input, "file_path", "write")?;
        let content = str_arg(input, "content", "write")?;
        let existed = self.put(path, content).await?;
        Ok(ToolOutcome::ok(format!("File {}: {path}", if existed { "updated" } else { "created" })))
    }

    pub(super) async fn edit(&self, input: &Value) -> Result<ToolOutcome, ToolOutcome> {
        let path = str_arg(input, "file_path", "edit")?;
        let old = str_arg(input, "old_string", "edit")?;
        let new = str_arg(input, "new_string", "edit")?;
        let all = crate::json::at(input, "/replace_all").as_bool().unwrap_or(false);
        let content = self.cat(path).await?;
        let (updated, n) = text::replace(&content, old, new, all).map_err(ToolOutcome::err)?;
        self.put(path, &updated).await?;
        Ok(ToolOutcome::ok(format!("File edited: {path} ({n} replacement{})", if n == 1 { "" } else { "s" })))
    }

    pub(super) async fn glob(&self, input: &Value) -> Result<ToolOutcome, ToolOutcome> {
        let pattern = str_arg(input, "pattern", "glob")?;
        let base = crate::json::at(input, "/path").as_str().unwrap_or(WORKDIR).trim_end_matches('/');
        let matcher = glob::Pattern::new(pattern).map_err(|e| ToolOutcome::err(format!("glob: {e}")))?;
        let out = self
            .run(shell(r#"find "$1" \( -type f -o -type l \) 2>/dev/null"#, &[base]), None, FILE_TIMEOUT)
            .await?;
        let opts = glob::MatchOptions { require_literal_separator: true, ..glob::MatchOptions::new() };
        let listing = String::from_utf8_lossy(&out.stdout);
        let mut hits: Vec<&str> = listing
            .lines()
            .filter(|p| {
                let rel = p.strip_prefix(base).unwrap_or(p).trim_start_matches('/');
                matcher.matches_with(rel, opts) || matcher.matches_with(p, opts)
            })
            .collect();
        hits.sort_unstable();
        hits.truncate(MAX_MATCHES);
        Ok(ToolOutcome::ok(if hits.is_empty() { "No files found".to_owned() } else { hits.join("\n") }))
    }

    pub(super) async fn grep(&self, input: &Value) -> Result<ToolOutcome, ToolOutcome> {
        let pattern = str_arg(input, "pattern", "grep")?;
        let path = crate::json::at(input, "/path").as_str().unwrap_or(WORKDIR);
        let mut flags = String::from("-rnE");
        if crate::json::at(input, "/case_insensitive").as_bool() == Some(true) {
            flags.push('i');
        }
        if crate::json::at(input, "/files_with_matches").as_bool() == Some(true) {
            flags.push('l');
        }
        let include = crate::json::at(input, "/glob").as_str().unwrap_or_default();
        let script = format!(
            "t=$(mktemp); grep {flags} ${{3:+--include=\"$3\"}} -- \"$1\" \"$2\" > \"$t\" 2>&1; s=$?; head -n {MAX_MATCHES} \"$t\"; rm -f \"$t\"; exit $s"
        );
        let out = self.run(shell(&script, &[pattern, path, include]), None, FILE_TIMEOUT).await?;
        match out.exit_code {
            0 => Ok(ToolOutcome::ok(String::from_utf8_lossy(&out.stdout).into_owned())),
            1 => Ok(ToolOutcome::ok("No matches found")),
            _ => Err(ToolOutcome::err(combined(&out).trim().to_owned())),
        }
    }
}
