//! Memory business logic — wire-exact behaviors + the versioning extension.

use std::sync::Arc;

use shared::ApiError;

use crate::store::{MemoryPersistence, StoreError};
use crate::types::{MemoryId, MemoryStoreId};

mod memories;
mod memory_reads;

pub use memory_reads::MemoryListQuery;
mod stores;
mod versions;

/// Read view: `basic` omits content, `full` includes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Metadata only (list default).
    Basic,
    /// Include content.
    Full,
}

/// Memory service.
pub struct MemoriesService {
    store: Arc<dyn MemoryPersistence>,
}

impl std::fmt::Debug for MemoriesService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoriesService").finish_non_exhaustive()
    }
}

impl MemoriesService {
    /// Build with a persistence backend.
    pub fn new(store: Arc<dyn MemoryPersistence>) -> Self {
        Self { store }
    }
}

fn parse_store_id(raw: &str) -> Result<MemoryStoreId, ApiError> {
    MemoryStoreId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn parse_mem_id(raw: &str) -> Result<MemoryId, ApiError> {
    MemoryId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn parse_mem_pair(s: &str, m: &str) -> Result<(MemoryStoreId, MemoryId), ApiError> {
    Ok((parse_store_id(s)?, parse_mem_id(m)?))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::StoreNotFound => ApiError::not_found("memory store not found"),
        StoreError::MemoryNotFound => ApiError::not_found("memory not found"),
        StoreError::VersionNotFound => ApiError::not_found("memory version not found"),
        StoreError::PathConflict { path, memory_id } => {
            ApiError::memory_path_conflict(&path, memory_id.as_str())
        }
        StoreError::HeadMoved => {
            ApiError::conflict_invalid_request("the memory changed while this request ran; retry it")
        }
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
