//! Memory wire types — all shapes captured live.

use agentd_core::{Actor, Id, Metadata, Resource, Timestamp};
use serde::{Deserialize, Serialize};

/// Marker for `memstore_…`.
#[derive(Debug, Clone, Copy)]
pub struct MemoryStoreResource;
impl Resource for MemoryStoreResource {
    const PREFIX: &'static str = "memstore";
    const LABEL: &'static str = "memory store";
}
/// `Id<MemoryStoreResource>`.
pub type MemoryStoreId = Id<MemoryStoreResource>;

/// Marker for `mem_…`.
#[derive(Debug, Clone, Copy)]
pub struct MemoryResource;
impl Resource for MemoryResource {
    const PREFIX: &'static str = "mem";
    const LABEL: &'static str = "memory";
}
/// `Id<MemoryResource>`.
pub type MemoryId = Id<MemoryResource>;

/// Marker for `memver_…`.
#[derive(Debug, Clone, Copy)]
pub struct MemoryVersionResource;
impl Resource for MemoryVersionResource {
    const PREFIX: &'static str = "memver";
    const LABEL: &'static str = "memory version";
}
/// `Id<MemoryVersionResource>`.
pub type MemoryVersionId = Id<MemoryVersionResource>;

/// Constant `"type":"memory_store"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStoreTag {
    /// Always `memory_store`.
    MemoryStore,
}

/// Constant `"type":"memory"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTag {
    /// Always `memory`.
    Memory,
}

/// Constant `"type":"memory_version"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryVersionTag {
    /// Always `memory_version`.
    MemoryVersion,
}

/// The store resource — live fixture:
/// `{created_at, description, id, metadata, name, type, updated_at}`
/// (note: **no** `archived_at` on the create response; it appears after
/// archive — modeled as skip-when-none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStoreEntity {
    /// Archive stamp — absent on the live create response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<Timestamp>,
    /// Created.
    pub created_at: Timestamp,
    /// Description — passed to the agent; write for the model.
    pub description: String,
    /// `memstore_…`.
    pub id: MemoryStoreId,
    /// User metadata.
    pub metadata: Metadata,
    /// Name.
    pub name: String,
    /// Always `"memory_store"`.
    #[serde(rename = "type")]
    pub tag: MemoryStoreTag,
    /// Updated.
    pub updated_at: Timestamp,
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

/// `POST /v1/memory_stores` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateStoreRequest {
    /// Required.
    pub name: Option<String>,
    /// Optional.
    pub description: Option<String>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
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

#[cfg(test)]
mod tests {
    use super::*;

    const MEM_FIXTURE: &str = include_str!("../../../fixtures/memory.create.json");
    const STORE_FIXTURE: &str = include_str!("../../../fixtures/memory_store.create.json");
    const VERSIONS_FIXTURE: &str = include_str!("../../../fixtures/memory_versions.list.json");

    #[test]
    fn memory_fixture_roundtrip() {
        let original: serde_json::Value = serde_json::from_str(MEM_FIXTURE).unwrap();
        let m: Memory = serde_json::from_str(MEM_FIXTURE).unwrap();
        assert_eq!(serde_json::to_value(&m).unwrap(), original);
    }

    #[test]
    fn store_fixture_roundtrip() {
        let original: serde_json::Value = serde_json::from_str(STORE_FIXTURE).unwrap();
        let s: MemoryStoreEntity = serde_json::from_str(STORE_FIXTURE).unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), original);
    }

    #[test]
    fn version_fixture_roundtrip() {
        let wrapper: serde_json::Value = serde_json::from_str(VERSIONS_FIXTURE).unwrap();
        let first = wrapper["data"][0].clone();
        let v: MemoryVersion = serde_json::from_value(first.clone()).unwrap();
        assert_eq!(serde_json::to_value(&v).unwrap(), first);
        assert_eq!(v.operation, VersionOperation::Created);
        assert!(matches!(v.created_by, Actor::ApiActor { .. }));
    }

    #[test]
    fn precondition_parses() {
        let p: Precondition =
            serde_json::from_str(r#"{"type":"content_sha256","content_sha256":"deadbeef"}"#).unwrap();
        assert!(matches!(p, Precondition::ContentSha256 { .. }));
    }
}
