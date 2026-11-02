//! The sandbox port over a session's work item: each command or file
//! transfer is queued for the worker in the sandbox, which long-polls for
//! it and posts the outcome back. The sandbox needs no inbound port.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sandbox_protocol::{decode, encode, DirOwner, Envelope, FileOut, Op, Outcome};
use shared::ports::sandbox::{ExecOutput, ExecRequest, Owner, Sandbox, SandboxFile};
use shared::ports::RuntimeError;

use crate::board::Board;

/// How long past a command's own timeout agentd waits for its outcome, and
/// how long a file transfer may take.
const GRACE: Duration = Duration::from_secs(120);

/// How often a waiting operation checks its worker still holds the lease.
const LEASE_CHECK: Duration = Duration::from_secs(5);

/// The sandbox of a session, reached through its worker.
#[derive(Debug, Clone)]
pub struct RemoteSandbox {
    board: Arc<Board>,
}

impl RemoteSandbox {
    /// Queue operations on `board`.
    pub fn new(board: Arc<Board>) -> Self {
        Self { board }
    }

    async fn run(&self, session: &str, op: Op, wait: Duration) -> Result<Outcome, RuntimeError> {
        let work = self.board.of_session(session).ok_or_else(|| RuntimeError::Unknown(session.to_owned()))?;
        let id = format!("op_{}", hex_id());
        let rx = self
            .board
            .enqueue(&work, Envelope { id, op })
            .ok_or_else(|| RuntimeError::Unknown(session.to_owned()))?;
        let deadline = tokio::time::Instant::now() + wait;
        tokio::pin!(rx);
        loop {
            // A worker whose lease ran out will not answer: fail now, not at the deadline.
            let tick = tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + LEASE_CHECK));
            tokio::select! {
                got = &mut rx => return match got {
                    Ok(Outcome::Error { message }) => Err(RuntimeError::Launch(message)),
                    Ok(outcome) => Ok(outcome),
                    Err(_) => Err(RuntimeError::Unknown(session.to_owned())),
                },
                () = tick => {
                    if tokio::time::Instant::now() >= deadline {
                        let secs = wait.as_secs();
                        return Err(RuntimeError::Unavailable(format!("the sandbox of {session} did not answer in {secs}s")));
                    }
                    if !self.board.alive(&work) {
                        return Err(RuntimeError::Unavailable(format!("the sandbox worker of {session} lost its lease")));
                    }
                }
            }
        }
    }
}

fn hex_id() -> String {
    rand::random::<[u8; 12]>().iter().map(|b| format!("{b:02x}")).collect()
}

fn bad(e: String) -> RuntimeError {
    RuntimeError::Unavailable(format!("the sandbox worker sent {e}"))
}

#[async_trait]
impl Sandbox for RemoteSandbox {
    async fn exec(&self, target_id: &str, req: ExecRequest) -> Result<ExecOutput, RuntimeError> {
        let op = Op::Exec {
            argv: req.argv,
            stdin: req.stdin.as_deref().map(encode),
            workdir: req.workdir,
            timeout_ms: u64::try_from(req.timeout.as_millis()).unwrap_or(u64::MAX),
            env: req.env,
        };
        match self.run(target_id, op, req.timeout + GRACE).await? {
            Outcome::Exec { exit_code, stdout, stderr, timed_out } => Ok(ExecOutput {
                exit_code,
                stdout: decode(&stdout).map_err(bad)?,
                stderr: decode(&stderr).map_err(bad)?,
                timed_out,
            }),
            other => Err(bad(format!("{other:?} for a command"))),
        }
    }

    async fn put_files(&self, target_id: &str, files: Vec<SandboxFile>) -> Result<(), RuntimeError> {
        if files.is_empty() {
            return Ok(());
        }
        let files = files
            .into_iter()
            .map(|f| FileOut { path: f.path, bytes: encode(&f.bytes), mode: f.mode })
            .collect();
        self.run(target_id, Op::PutFiles { files }, GRACE).await.map(|_| ())
    }

    async fn make_dir(&self, target_id: &str, dir: &str, owner: Owner) -> Result<(), RuntimeError> {
        let owner = match owner {
            Owner::Root => DirOwner::Root,
            Owner::Agent => DirOwner::Agent,
        };
        self.run(target_id, Op::MakeDir { dir: dir.to_owned(), owner }, GRACE).await.map(|_| ())
    }

    async fn read_tree(&self, target_id: &str, dir: &str) -> Result<BTreeMap<String, Vec<u8>>, RuntimeError> {
        match self.run(target_id, Op::ReadTree { dir: dir.to_owned() }, GRACE).await? {
            Outcome::Tree { files } => {
                files.into_iter().map(|(p, b)| decode(&b).map(|b| (p, b)).map_err(bad)).collect()
            }
            other => Err(bad(format!("{other:?} for a read"))),
        }
    }
}
