//! Fakes for tests: a scripted sandbox (files in memory, each command
//! answered by a handler), an in-memory session with its event log, and
//! in-memory assets.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use shared::ports::sandbox::{ExecOutput, ExecRequest, Owner, Sandbox, SandboxFile};
use shared::ports::RuntimeError;

mod assets;
mod session;

pub use assets::{FakeAssets, SavedOutput};
pub use session::FakeSession;

type Handler = dyn Fn(&ExecRequest) -> ExecOutput + Send + Sync;

/// An in-memory [`Sandbox`].
#[derive(Clone)]
pub struct FakeSandbox {
    handler: Arc<Handler>,
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    dirs: Arc<Mutex<Vec<(String, Owner)>>>,
    execs: Arc<Mutex<Vec<ExecRequest>>>,
}

impl std::fmt::Debug for FakeSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeSandbox").finish_non_exhaustive()
    }
}

impl FakeSandbox {
    /// Answer every command with `handler`.
    pub fn new(handler: impl Fn(&ExecRequest) -> ExecOutput + Send + Sync + 'static) -> Self {
        Self {
            handler: Arc::new(handler),
            files: Arc::default(),
            dirs: Arc::default(),
            execs: Arc::default(),
        }
    }

    /// Answer every command with exit 0 and `stdout`.
    pub fn answering(stdout: &str) -> Self {
        let stdout = stdout.as_bytes().to_vec();
        Self::new(move |_| ExecOutput { stdout: stdout.clone(), ..ExecOutput::default() })
    }

    /// The commands run so far.
    pub fn execs(&self) -> Vec<ExecRequest> {
        self.execs.lock().clone()
    }

    /// The files put so far, by path.
    pub fn files(&self) -> BTreeMap<String, Vec<u8>> {
        self.files.lock().clone()
    }

    /// The directories made so far.
    pub fn dirs(&self) -> Vec<(String, Owner)> {
        self.dirs.lock().clone()
    }

    /// Put a file as if the agent had written it.
    pub fn seed(&self, path: &str, bytes: &[u8]) {
        self.files.lock().insert(path.to_owned(), bytes.to_vec());
    }
}

#[async_trait]
impl Sandbox for FakeSandbox {
    async fn exec(&self, _target_id: &str, req: ExecRequest) -> Result<ExecOutput, RuntimeError> {
        if let [test, flag, path] = req.argv.as_slice() {
            if test == "test" && flag == "-f" {
                return Ok(ExecOutput {
                    exit_code: i64::from(!self.files.lock().contains_key(path)),
                    ..ExecOutput::default()
                });
            }
        }
        let out = (self.handler)(&req);
        self.execs.lock().push(req);
        Ok(out)
    }

    async fn put_files(&self, _target_id: &str, files: Vec<SandboxFile>) -> Result<(), RuntimeError> {
        let mut held = self.files.lock();
        for f in files {
            held.insert(f.path, f.bytes);
        }
        Ok(())
    }

    async fn make_dir(&self, _target_id: &str, dir: &str, owner: Owner) -> Result<(), RuntimeError> {
        self.dirs.lock().push((dir.to_owned(), owner));
        Ok(())
    }

    async fn read_tree(
        &self,
        _target_id: &str,
        dir: &str,
    ) -> Result<BTreeMap<String, Vec<u8>>, RuntimeError> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        Ok(self
            .files
            .lock()
            .iter()
            .filter(|(p, _)| p.starts_with(&prefix))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect())
    }
}
