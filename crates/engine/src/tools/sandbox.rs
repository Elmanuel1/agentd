//! The toolset's file and shell tools, run inside the session's sandbox as
//! the agent's user, so the sandbox's permissions decide what they may touch.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use shared::ports::sandbox::{ExecOutput, ExecRequest, Sandbox};

use super::text::{self, OUTPUT_LIMIT};
use super::web::WebTools;
use super::{ToolCall, ToolExecutor, ToolOutcome};

/// Where commands start until a `cd` moves a thread's shell.
pub const WORKDIR: &str = "/workspace";

/// How long one bash command may run.
const BASH_TIMEOUT: Duration = Duration::from_secs(600);

/// How long a file operation may run.
pub(super) const FILE_TIMEOUT: Duration = Duration::from_secs(60);

/// Most glob or grep results returned.
pub(super) const MAX_MATCHES: usize = 1000;

/// The toolset over one session's sandbox.
#[derive(Clone)]
pub struct SandboxTools {
    sandbox: Arc<dyn Sandbox>,
    target: String,
    web: WebTools,
}

impl std::fmt::Debug for SandboxTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxTools").field("target", &self.target).finish_non_exhaustive()
    }
}

/// Run `script` with bash where the image has it, else sh; `$1…` are `args`.
pub(super) fn shell(script: &str, args: &[&str]) -> Vec<String> {
    let mut argv = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        r#"s=$1; shift; if command -v bash >/dev/null 2>&1; then exec bash -c "$s" bash "$@"; else exec sh -c "$s" sh "$@"; fi"#.to_owned(),
        "sh".to_owned(),
        script.to_owned(),
    ];
    argv.extend(args.iter().map(|a| (*a).to_owned()));
    argv
}

pub(super) fn str_arg<'a>(input: &'a Value, key: &str, tool: &str) -> Result<&'a str, ToolOutcome> {
    input[key].as_str().ok_or_else(|| ToolOutcome::err(format!("{tool}: `{key}` is required")))
}

pub(super) fn combined(out: &ExecOutput) -> String {
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

impl SandboxTools {
    /// Run tools in `target`'s sandbox; the web tools go through `web`.
    pub fn new(sandbox: Arc<dyn Sandbox>, target: impl Into<String>, web: WebTools) -> Self {
        Self { sandbox, target: target.into(), web }
    }

    pub(super) async fn run(
        &self,
        argv: Vec<String>,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<ExecOutput, ToolOutcome> {
        let mut req = ExecRequest::new(argv, timeout).in_dir(WORKDIR);
        if let Some(bytes) = stdin {
            req = req.with_stdin(bytes);
        }
        self.sandbox.exec(&self.target, req).await.map_err(|e| ToolOutcome::err(format!("sandbox: {e}")))
    }

    async fn bash(&self, call: &ToolCall) -> Result<ToolOutcome, ToolOutcome> {
        let command = str_arg(&call.input, "command", "bash")?;
        let state = format!("/tmp/.agentd/shell-{}.cwd", call.thread);
        let script = format!(
            "mkdir -p /tmp/.agentd 2>/dev/null; __d=$(cat {state} 2>/dev/null); cd \"${{__d:-{WORKDIR}}}\" 2>/dev/null || cd {WORKDIR} 2>/dev/null\n{command}\n__s=$?; pwd > {state} 2>/dev/null; exit $__s"
        );
        let out = self.run(shell(&script, &[]), None, BASH_TIMEOUT).await?;
        if out.timed_out {
            return Ok(ToolOutcome::err(format!(
                "{}\nCommand timed out after {}s",
                combined(&out),
                BASH_TIMEOUT.as_secs()
            )));
        }
        let mut text = combined(&out);
        if out.exit_code != 0 {
            text.push_str(&format!("\nExit code: {}", out.exit_code));
        }
        Ok(ToolOutcome::ok(text))
    }

    /// Save an output too large for the model to a file, and point at it.
    async fn spill(&self, call: &ToolCall, outcome: ToolOutcome) -> ToolOutcome {
        if outcome.output.len() <= OUTPUT_LIMIT {
            return outcome;
        }
        let path = format!("/tmp/agentd-tool-outputs/{}.txt", call.id);
        let note = match self.put(&path, &outcome.output).await {
            Ok(_) => format!(
                "\n\n[Output truncated: {} characters. The full output is in {path}.]",
                outcome.output.chars().count()
            ),
            Err(e) => format!("\n\n[Output truncated; saving it failed: {}]", e.output),
        };
        ToolOutcome {
            output: format!("{}{note}", text::preview(&outcome.output)),
            is_error: outcome.is_error,
        }
    }
}

#[async_trait]
impl ToolExecutor for SandboxTools {
    async fn execute(&self, call: &ToolCall) -> ToolOutcome {
        let result = match call.name.as_str() {
            "bash" => self.bash(call).await,
            "read" => self.read(&call.input).await,
            "write" => self.write(&call.input).await,
            "edit" => self.edit(&call.input).await,
            "glob" => self.glob(&call.input).await,
            "grep" => self.grep(&call.input).await,
            "web_fetch" => Ok(self.web.fetch(&call.input).await),
            "web_search" => Ok(self.web.search(&call.input).await),
            other => Err(ToolOutcome::err(format!("unknown tool: {other}"))),
        };
        let outcome = result.unwrap_or_else(|e| e);
        self.spill(call, outcome).await
    }
}

#[cfg(test)]
mod tests;
