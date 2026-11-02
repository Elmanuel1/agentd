//! The memory lists' filters.

use shared::page::filters::CreatedRange;

/// The memory-versions list's filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionFilter {
    /// Only this memory's versions.
    pub memory_id: Option<String>,
    /// Only versions written by this API key.
    pub api_key_id: Option<String>,
    /// Only versions a session wrote.
    pub session_id: Option<String>,
    /// Only versions a service account wrote (agentd has none).
    pub service_account_id: Option<String>,
    /// Only this operation (`created`, `modified`, `deleted`).
    pub operation: Option<crate::types::VersionOperation>,
    /// Creation-time bounds.
    pub created: CreatedRange,
}

/// The memory-stores list's filters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreFilter {
    /// Keep archived stores.
    pub include_archived: bool,
    /// Creation-time bounds.
    pub created: CreatedRange,
}
