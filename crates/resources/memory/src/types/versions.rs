use serde::{Deserialize, Serialize};
use shared::{Actor, Timestamp};

use super::{MemoryId, MemoryStoreId, MemoryVersionId};

/// Constant `"type":"memory_version"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryVersionTag {
    /// Always `memory_version`.
    MemoryVersion,
}

/// The version operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionOperation {
    /// First write at a path.
    Created,
    /// Content and/or path change.
    Modified,
    /// Memory deleted.
    Deleted,
}

/// An immutable version record — live fixture:
/// `{content_sha256, content_size_bytes, created_at, created_by, id,
///   memory_id, memory_store_id, operation, path, redacted_by, type}`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryVersion {
    /// Full content — only on `view=full` reads; cleared by redact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Content hash (nulled by redact).
    pub content_sha256: Option<String>,
    /// Content size (nulled by redact).
    pub content_size_bytes: Option<u64>,
    /// Created.
    pub created_at: Timestamp,
    /// Who made the mutation.
    pub created_by: Actor,
    /// `memver_…`.
    pub id: MemoryVersionId,
    /// The memory this version belongs to.
    pub memory_id: MemoryId,
    /// The store.
    pub memory_store_id: MemoryStoreId,
    /// The operation.
    pub operation: VersionOperation,
    /// Path at the time of the mutation (nulled by redact).
    pub path: Option<String>,
    /// Who redacted, if redacted.
    pub redacted_by: Option<Actor>,
    /// Always `"memory_version"`.
    #[serde(rename = "type")]
    pub tag: MemoryVersionTag,
}
