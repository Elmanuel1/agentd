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

impl VersionFilter {
    /// Whether `v` passes every filter.
    pub fn keeps(&self, v: &crate::types::MemoryVersion) -> bool {
        use shared::Actor;
        let by_key =
            |id: &String| matches!(&v.created_by, Actor::ApiActor { api_key_id } if api_key_id == id);
        let by_session =
            |id: &String| matches!(&v.created_by, Actor::SessionActor { session_id } if session_id == id);
        self.api_key_id.as_ref().is_none_or(by_key)
            && self.session_id.as_ref().is_none_or(by_session)
            && self.service_account_id.is_none()
            && self.operation.is_none_or(|op| op == v.operation)
            && self.created.contains(v.created_at)
    }
}

/// The memory-stores list's filters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreFilter {
    /// Keep archived stores.
    pub include_archived: bool,
    /// Creation-time bounds.
    pub created: CreatedRange,
}
