use serde::{Deserialize, Serialize};
use shared::{Id, Resource, Timestamp};

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
        /// Attached.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<Timestamp>,
        /// Last changed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_at: Option<Timestamp>,
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
        /// Attached.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<Timestamp>,
        /// Last changed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_at: Option<Timestamp>,
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

impl SessionResourceInput {
    /// The attached resource, with its id, its default mount path and its
    /// timestamps. A repository's token is dropped here and never stored.
    pub fn into_resource(self, now: Timestamp) -> SessionResource {
        let id = Some(SessionResourceId::generate());
        match self {
            Self::File { file_id, mount_path } => SessionResource::File {
                id,
                mount_path: mount_path.unwrap_or_else(|| format!("/workspace/{file_id}")),
                file_id,
                created_at: Some(now),
                updated_at: Some(now),
            },
            Self::GithubRepository { url, mount_path, checkout, .. } => SessionResource::GithubRepository {
                id,
                mount_path: Some(mount_path.unwrap_or_else(|| {
                    let repo = url.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
                    format!("/workspace/{}", repo.trim_end_matches(".git"))
                })),
                url,
                checkout,
                created_at: Some(now),
                updated_at: Some(now),
            },
            Self::MemoryStore { memory_store_id, access, instructions } => {
                SessionResource::MemoryStore { id, memory_store_id, access, instructions }
            }
        }
    }
}

impl SessionResource {
    /// The resource's id (memory stores attached before ids were assigned have none).
    pub fn id(&self) -> Option<&SessionResourceId> {
        match self {
            Self::File { id, .. } | Self::GithubRepository { id, .. } | Self::MemoryStore { id, .. } => {
                id.as_ref()
            }
        }
    }
}

impl super::Session {
    /// Whether the session mounts the file `file_id` as a resource.
    #[must_use]
    pub fn mounts(&self, file_id: &str) -> bool {
        self.resources.iter().any(|r| matches!(r, SessionResource::File { file_id: f, .. } if f == file_id))
    }
}
