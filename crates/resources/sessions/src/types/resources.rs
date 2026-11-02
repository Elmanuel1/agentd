use serde::{Deserialize, Serialize};
use shared::{Id, Resource};

/// Marker for `sesrsc_…` session-resource ids.
#[derive(Debug, Clone, Copy)]
pub struct SessionResourceEntry_;
impl Resource for SessionResourceEntry_ {
    const PREFIX: &'static str = "sesrsc";
    const LABEL: &'static str = "session resource";
}
/// `Id<SessionResourceEntry_>` alias.
pub type SessionResourceId = Id<SessionResourceEntry_>;

/// Repo checkout selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Checkout {
    /// A branch by name.
    Branch {
        /// Branch name.
        name: String,
    },
    /// A commit by sha.
    Commit {
        /// Commit sha.
        sha: String,
    },
}

/// Memory-store access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAccess {
    /// Read/write (default).
    ReadWrite,
    /// Read-only mount.
    ReadOnly,
}

/// A session resource — file / repo / memory store. Cross-entity links are
/// by ID string (no crate dependency on files/memory).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionResource {
    /// An uploaded file mounted read-only.
    File {
        /// `sesrsc_…` (assigned at attach).
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<SessionResourceId>,
        /// The uploaded file (session-scoped copy).
        file_id: String,
        /// Absolute mount path.
        mount_path: String,
    },
    /// A GitHub repository cloned at startup.
    GithubRepository {
        /// `sesrsc_…`.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<SessionResourceId>,
        /// Repo URL.
        url: String,
        /// Clone path (defaults to /workspace/<repo>).
        #[serde(skip_serializing_if = "Option::is_none")]
        mount_path: Option<String>,
        /// Checkout selector.
        #[serde(skip_serializing_if = "Option::is_none")]
        checkout: Option<Checkout>,
        // NOTE: authorization_token is accepted on input but NEVER echoed —
        // it is stripped before this type is constructed (see requests).
    },
    /// A memory store mount (session-create only).
    MemoryStore {
        /// `sesrsc_…`.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<SessionResourceId>,
        /// The store.
        memory_store_id: String,
        /// Access mode.
        #[serde(skip_serializing_if = "Option::is_none")]
        access: Option<MemoryAccess>,
        /// Session-specific guidance (≤4096).
        #[serde(skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
    },
}

/// Input resource shape — carries the write-only `authorization_token` for
/// repos, which is stripped before the wire type is built.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionResourceInput {
    /// File mount.
    File {
        /// Uploaded file id.
        file_id: String,
        /// Required absolute mount path.
        mount_path: Option<String>,
    },
    /// Repo mount.
    GithubRepository {
        /// Repo URL.
        url: String,
        /// Clone token (write-only — never echoed).
        #[allow(dead_code)]
        authorization_token: Option<String>,
        /// Clone path.
        mount_path: Option<String>,
        /// Checkout.
        checkout: Option<Checkout>,
    },
    /// Memory store mount (session-create only).
    MemoryStore {
        /// Store id.
        memory_store_id: String,
        /// Access.
        access: Option<MemoryAccess>,
        /// Instructions.
        instructions: Option<String>,
    },
}
