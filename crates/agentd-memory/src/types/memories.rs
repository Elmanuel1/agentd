use agentd_core::Timestamp;
use serde::{Deserialize, Serialize};

use super::{MemoryId, MemoryStoreId, MemoryVersionId};

/// Constant `"type":"memory"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTag {
    /// Always `memory`.
    Memory,
}

/// A memory head — live fixture:
/// `{content_sha256, content_size_bytes, created_at, id, memory_store_id,
///   memory_version_id, path, type, updated_at}`
/// `content` appears only on full-view reads (create/update do NOT echo it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    /// Full content — only on `view=full` reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Lowercase-hex SHA-256 of the content.
    pub content_sha256: String,
    /// Content size.
    pub content_size_bytes: u64,
    /// Created.
    pub created_at: Timestamp,
    /// `mem_…`.
    pub id: MemoryId,
    /// Parent store.
    pub memory_store_id: MemoryStoreId,
    /// **The moving head pointer** — the `latest` ref target.
    pub memory_version_id: MemoryVersionId,
    /// Unique path within the store (e.g. `/rules.md`).
    pub path: String,
    /// Always `"memory"`.
    #[serde(rename = "type")]
    pub tag: MemoryTag,
    /// Updated.
    pub updated_at: Timestamp,
}

/// Constant `"type":"memory_prefix"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPrefixTag {
    /// Always `memory_prefix`.
    MemoryPrefix,
}

/// A collapsed sub-directory in a `depth`-limited memory listing — live shape
/// `{"path":"/notes/","type":"memory_prefix"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPrefix {
    /// The directory path, trailing-slash terminated.
    pub path: String,
    /// Always `"memory_prefix"`.
    #[serde(rename = "type")]
    pub tag: MemoryPrefixTag,
}

/// One entry in a memory listing: a leaf `memory` or a collapsed
/// `memory_prefix`. Serialized untagged (each variant carries its own `type`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum MemoryListItem {
    /// A leaf memory (content omitted in listings).
    Memory(Box<Memory>),
    /// A collapsed sub-directory.
    Prefix(MemoryPrefix),
}

/// The list-memories response envelope — live shape `{data, prefixes}`.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryListPage {
    /// Memories and collapsed prefixes at this depth.
    pub data: Vec<MemoryListItem>,
    /// Always `[]` in the current API (collapsed dirs live in `data`).
    pub prefixes: Vec<MemoryPrefix>,
}

/// Optimistic-concurrency precondition on update/delete.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Precondition {
    /// Current content must hash to this value.
    ContentSha256 {
        /// Expected lowercase-hex sha256 (64 chars).
        content_sha256: String,
    },
}

/// `POST /v1/memory_stores/{id}/memories` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMemoryRequest {
    /// Required — unique path.
    pub path: Option<String>,
    /// Required — ≤102400 bytes.
    pub content: Option<String>,
}

/// `POST /v1/memory_stores/{sid}/memories/{mid}` body (update via POST —
/// live-verified; PATCH is 405).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateMemoryRequest {
    /// New content.
    pub content: Option<String>,
    /// Rename.
    pub path: Option<String>,
    /// Optimistic lock.
    pub precondition: Option<Precondition>,
}
