use serde::{Deserialize, Serialize};
use shared::{Metadata, MetadataPatch, Patch, Timestamp};

use super::MemoryStoreId;

/// Constant `"type":"memory_store"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStoreTag {
    /// Always `memory_store`.
    MemoryStore,
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

/// `POST /v1/memory_stores/{id}` body: a field left out keeps its value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateStoreRequest {
    /// The new name (`null` keeps it).
    #[serde(default)]
    pub name: Option<String>,
    /// The new description; `null` or `""` clears it.
    #[serde(default)]
    pub description: Patch<String>,
    /// Merge: strings upsert, `null` removes.
    #[serde(default)]
    pub metadata: Patch<MetadataPatch>,
}
