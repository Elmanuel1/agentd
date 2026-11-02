//! Memory wire types — all shapes captured live.

use shared::{Id, Resource};

mod memories;
mod stores;
mod versions;

pub use memories::{
    CreateMemoryRequest, Memory, MemoryListItem, MemoryListPage, MemoryPrefix, MemoryPrefixTag, MemoryTag,
    Precondition, UpdateMemoryRequest,
};
pub use stores::{CreateStoreRequest, MemoryStoreEntity, MemoryStoreTag};
pub use versions::{MemoryVersion, MemoryVersionTag, VersionOperation};

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

#[cfg(test)]
mod tests;
