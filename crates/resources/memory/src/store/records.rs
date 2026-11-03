//! A memory's head and version chain, as the port reads and writes them.

use crate::types::{Memory, MemoryVersion, MemoryVersionId};

/// How a reader resolves a memory to a concrete version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSelector {
    /// The current head — re-resolved at each session-create.
    Latest,
    /// A pinned version.
    Pinned(MemoryVersionId),
}

/// One version's full record: wire metadata + content blob.
#[derive(Debug, Clone)]
pub struct VersionRecord {
    /// Wire metadata (content field unset; populated on full view).
    pub version: MemoryVersion,
    /// The content blob — `None` after redact.
    pub blob: Option<String>,
}

/// A memory's full state: head pointer + chain.
#[derive(Debug, Clone)]
pub struct MemoryRecord {
    /// Wire head (its `memory_version_id` IS the `latest` ref).
    pub head: Memory,
    /// Whether the memory is deleted (chain retained for audit).
    pub deleted: bool,
    /// The versions read or written with the head, oldest first: on a read,
    /// the current version only.
    pub chain: Vec<VersionRecord>,
}

impl MemoryRecord {
    /// Find a version in the chain.
    fn find(&self, id: &MemoryVersionId) -> Option<&VersionRecord> {
        self.chain.iter().find(|v| &v.version.id == id)
    }

    /// Resolve a selector to a version record.
    pub fn resolve(&self, sel: &VersionSelector) -> Option<&VersionRecord> {
        match sel {
            VersionSelector::Latest => self.find(&self.head.memory_version_id),
            VersionSelector::Pinned(id) => self.find(id),
        }
    }
}
