//! Running one operation: commands as the agent's user, with a clean
//! environment (the worker's own, with its token, never reaches them), and
//! file transfers as root.

use std::collections::BTreeMap;
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sandbox_protocol::{decode, encode, DirOwner, FileOut, Op, Outcome};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Most bytes one read returns.
const MAX_TREE: u64 = 256 * 1024 * 1024;
/// Most bytes of stdout plus stderr kept from one command.
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

/// Who commands run as.
#[derive(Debug, Clone, Copy)]
pub struct Agent {
    /// The user id.
    pub uid: u32,
    /// The group id.
    pub gid: u32,
}

/// Run `op`.
pub async fn run(op: Op, agent: Agent) -> Outcome {
    let result = match op {
        Op::Exec { argv, stdin, workdir, timeout_ms, env } => {
            exec(argv, stdin, workdir, timeout_ms, env, agent).await
        }
        Op::PutFiles { files } => put_files(&files),
        Op::MakeDir { dir, owner } => make_dir(&dir, owner, agent),
        Op::ReadTree { dir } => read_tree(&dir),
    };
    result.unwrap_or_else(|message| Outcome::Error { message })
}

async fn exec(
    argv: Vec<String>,
    stdin: Option<String>,
    workdir: Option<String>,
    timeout_ms: u64,
    env: BTreeMap<String, String>,
    agent: Agent,
) -> Result<Outcome, String> {
    let (program, args) = argv.split_first().ok_or("exec: empty argv")?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env_clear()
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .env("HOME", "/workspace")
        .env("LANG", "C.UTF-8")
        .envs(env)
        .uid(agent.uid)
        .gid(agent.gid)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = workdir.filter(|d| Path::new(d).is_dir()) {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| format!("exec {program}: {e}"))?;
    let pgid = child.id();
    let input = stdin.map(|s| decode(&s)).transpose()?;
    let mut pipe = child.stdin.take();
    tokio::spawn(async move {
        if let (Some(p), Some(bytes)) = (pipe.as_mut(), input) {
            let _ = p.write_all(&bytes).await;
        }
        drop(pipe);
    });
    let (mut out, mut err) = (child.stdout.take(), child.stderr.take());
    let read = async {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        if let Some(s) = out.as_mut() {
            let _ = s.take(MAX_OUTPUT as u64).read_to_end(&mut o).await;
        }
        if let Some(s) = err.as_mut() {
            let _ = s.take(MAX_OUTPUT as u64).read_to_end(&mut e).await;
        }
        let status = child.wait().await;
        (o, e, status)
    };
    match tokio::time::timeout(Duration::from_millis(timeout_ms), read).await {
        Ok((o, e, status)) => {
            let exit_code = status.ok().and_then(|s| s.code()).map_or(-1, i64::from);
            Ok(Outcome::Exec { exit_code, stdout: encode(&o), stderr: encode(&e), timed_out: false })
        }
        Err(_) => {
            // The whole process group: a command's own children go with it.
            if let Some(pid) = pgid {
                kill_group(pid);
            }
            Ok(Outcome::Exec {
                exit_code: 137,
                stdout: String::new(),
                stderr: encode(b"killed: timed out"),
                timed_out: true,
            })
        }
    }
}

/// Kill process group `pgid` with the `kill` program: the workspace forbids
/// `unsafe`, and so `libc::kill`.
fn kill_group(pgid: u32) {
    let _ = std::process::Command::new("kill").args(["-KILL", "--", &format!("-{pgid}")]).status();
}

fn io(path: &str, e: std::io::Error) -> String {
    format!("{path}: {e}")
}

fn put_files(files: &[FileOut]) -> Result<Outcome, String> {
    for f in files {
        let path = Path::new(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(&f.path, e))?;
        }
        std::fs::write(path, decode(&f.bytes)?).map_err(|e| io(&f.path, e))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(f.mode))
            .map_err(|e| io(&f.path, e))?;
        chown(path, Some(0), Some(0)).map_err(|e| io(&f.path, e))?;
    }
    Ok(Outcome::Done {})
}

fn make_dir(dir: &str, owner: DirOwner, agent: Agent) -> Result<Outcome, String> {
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let (uid, gid) = match owner {
        DirOwner::Root => (0, 0),
        DirOwner::Agent => (agent.uid, agent.gid),
    };
    chown(dir, Some(uid), Some(gid)).map_err(|e| io(dir, e))?;
    Ok(Outcome::Done {})
}

fn read_tree(dir: &str) -> Result<Outcome, String> {
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    let mut stack = vec![std::path::PathBuf::from(dir)];
    while let Some(p) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if meta.is_dir() {
            for entry in std::fs::read_dir(&p).map_err(|e| io(dir, e))?.flatten() {
                stack.push(entry.path());
            }
        } else if meta.is_file() {
            total += meta.len();
            if total > MAX_TREE {
                return Err(format!("{dir}: more than {MAX_TREE} bytes"));
            }
            let bytes = std::fs::read(&p).map_err(|e| io(dir, e))?;
            files.insert(p.to_string_lossy().into_owned(), encode(&bytes));
        }
    }
    Ok(Outcome::Tree { files })
}

#[cfg(test)]
mod tests;
