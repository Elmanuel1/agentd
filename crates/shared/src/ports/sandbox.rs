//! The sandbox port: run commands and move files inside a session's sandbox.
//! The agent loop runs in agentd and reaches the sandbox only through this
//! port, so the model API key and vault secrets never enter it.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;

use super::RuntimeError;

/// One command run inside a sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    /// The program and its arguments.
    pub argv: Vec<String>,
    /// Bytes written to the command's stdin, which is then closed.
    pub stdin: Option<Vec<u8>>,
    /// The working directory; the image's default when `None`.
    pub workdir: Option<String>,
    /// The command is killed after this long.
    pub timeout: Duration,
}

impl ExecRequest {
    /// Run `argv` with no stdin, killed after `timeout`.
    pub fn new(argv: impl IntoIterator<Item = impl Into<String>>, timeout: Duration) -> Self {
        Self { argv: argv.into_iter().map(Into::into).collect(), stdin: None, workdir: None, timeout }
    }

    /// Feed `bytes` to stdin.
    #[must_use]
    pub fn with_stdin(mut self, bytes: Vec<u8>) -> Self {
        self.stdin = Some(bytes);
        self
    }

    /// Run in `dir`.
    #[must_use]
    pub fn in_dir(mut self, dir: impl Into<String>) -> Self {
        self.workdir = Some(dir.into());
        self
    }
}

/// What a command produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    /// The exit code (`-1` when it is unknown).
    pub exit_code: i64,
    /// Everything written to stdout.
    pub stdout: Vec<u8>,
    /// Everything written to stderr.
    pub stderr: Vec<u8>,
    /// The command ran past its timeout and was killed.
    pub timed_out: bool,
}

/// Who owns a directory made in a sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// `root`; the agent can read it but not add to it.
    Root,
    /// The user the agent's commands run as.
    Agent,
}

/// A file to write into a sandbox, at an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxFile {
    /// Absolute path inside the sandbox.
    pub path: String,
    /// The contents.
    pub bytes: Vec<u8>,
    /// Unix permission bits.
    pub mode: u32,
}

/// Commands and files inside the sandbox a target (a session) runs in.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Run one command in `target_id`'s sandbox as the agent's user, so the
    /// sandbox's own permissions apply to everything the agent does.
    async fn exec(&self, target_id: &str, req: ExecRequest) -> Result<ExecOutput, RuntimeError>;
    /// Write `files` owned by root and readable by all (mounted uploads and
    /// skills, which the agent reads but cannot change). Missing parent
    /// directories are created owned by root.
    async fn put_files(&self, target_id: &str, files: Vec<SandboxFile>) -> Result<(), RuntimeError>;
    /// Create `dir` and its parents, with `dir` itself owned by `owner`.
    async fn make_dir(&self, target_id: &str, dir: &str, owner: Owner) -> Result<(), RuntimeError>;
    /// Every regular file under `dir`, by absolute path; empty when `dir` does
    /// not exist.
    async fn read_tree(&self, target_id: &str, dir: &str) -> Result<BTreeMap<String, Vec<u8>>, RuntimeError>;
}
