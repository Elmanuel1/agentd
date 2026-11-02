//! The sandbox operations agentd queues for a session's work item and the
//! in-sandbox worker runs (an agentd extension of the environments work
//! API: a hosted session's sandbox needs file transfer as well as commands).
//! Bytes travel as standard base64.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Bytes as standard base64.
pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Standard base64 back to bytes.
pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(s).map_err(|e| e.to_string())
}

/// Who owns a directory the worker makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirOwner {
    /// root; the agent can read it but not add to it.
    Root,
    /// The agent's user.
    Agent,
}

/// A file to write, owned by root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOut {
    /// Absolute path.
    pub path: String,
    /// The contents, base64.
    pub bytes: String,
    /// Unix permission bits.
    pub mode: u32,
}

/// One operation for the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Op {
    /// Run a command as the agent's user.
    Exec {
        /// Program and arguments.
        argv: Vec<String>,
        /// Bytes for stdin, base64.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdin: Option<String>,
        /// Working directory.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
        /// Killed after this many milliseconds.
        timeout_ms: u64,
        /// Extra environment variables.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
    },
    /// Write root-owned files, making missing parents as root.
    PutFiles {
        /// The files.
        files: Vec<FileOut>,
    },
    /// Make a directory and its parents.
    MakeDir {
        /// The directory.
        dir: String,
        /// Who owns it.
        owner: DirOwner,
    },
    /// Read every regular file under a directory (or the one file named).
    ReadTree {
        /// The directory or file.
        dir: String,
    },
}

/// An operation as the worker receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// The operation's id, echoed with its result.
    pub id: String,
    /// The operation.
    #[serde(flatten)]
    pub op: Op,
}

/// What an operation produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    /// A command ran.
    Exec {
        /// Its exit code (`-1` when unknown).
        exit_code: i64,
        /// Stdout, base64.
        stdout: String,
        /// Stderr, base64.
        stderr: String,
        /// It ran past its timeout and was killed.
        timed_out: bool,
    },
    /// Files or a directory were written.
    Done {},
    /// The files read, by absolute path, base64.
    Tree {
        /// The files.
        files: BTreeMap<String, String>,
    },
    /// The operation failed.
    Error {
        /// Why.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exec_op_has_a_flat_wire_shape() {
        let env = Envelope {
            id: "op_1".into(),
            op: Op::Exec {
                argv: vec!["ls".into()],
                stdin: None,
                workdir: None,
                timeout_ms: 1000,
                env: BTreeMap::new(),
            },
        };
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v, serde_json::json!({"id": "op_1", "type": "exec", "argv": ["ls"], "timeout_ms": 1000}));
        assert_eq!(serde_json::from_value::<Envelope>(v).unwrap(), env);
        assert_eq!(decode(&encode(b"\x00hi")).unwrap(), b"\x00hi");
    }
}
