//! `MemoryPersistence` on Postgres via Diesel — the git-object model:
//! `memory_stores` → `memories` (head) → `memory_versions` (append-only chain).
//! Version CONTENT is a blob in object storage, keyed by version id.
use std::sync::Arc;

use blobs_s3::BlobStore;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use memory::store::StoreError;
use memory::types::{MemoryStoreId, MemoryVersionId};
use shared::WorkspaceId;

use crate::schema::memory_stores;
use crate::{diesel_be, pool_be, PgBackend};

mod memories;
mod stores;
mod versions;

/// Postgres + object-storage backed memory store (Diesel).
#[derive(Clone)]
pub struct PgMemoryStore {
    backend: PgBackend,
    blobs: Arc<dyn BlobStore>,
}

impl std::fmt::Debug for PgMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgMemoryStore").finish_non_exhaustive()
    }
}

impl PgMemoryStore {
    /// Wrap a backend + blob store.
    pub fn new(backend: PgBackend, blobs: Arc<dyn BlobStore>) -> Self {
        Self { backend, blobs }
    }

    fn blob_key(ws: &WorkspaceId, store: &MemoryStoreId, version: &MemoryVersionId) -> String {
        format!("memory/{}/{}/{}", ws.as_str(), store.as_str(), version.as_str())
    }

    async fn require_store(&self, ws: &WorkspaceId, store: &MemoryStoreId) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = memory_stores::table
            .filter(memory_stores::workspace_id.eq(ws.as_str()))
            .filter(memory_stores::store_id.eq(store.as_str()))
            .select(memory_stores::store_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        exists.map(|_| ()).ok_or(StoreError::StoreNotFound)
    }
}
