use shared::limits::required;
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, Pagination, Timestamp};

use super::{map_store, parse_store_id, MemoriesService};
use crate::types::{CreateStoreRequest, MemoryStoreEntity, MemoryStoreId, MemoryStoreTag};

impl MemoriesService {
    /// `POST /v1/memory_stores`.
    pub async fn create_store(
        &self,
        ctx: &AuthContext,
        req: CreateStoreRequest,
    ) -> Result<MemoryStoreEntity, ApiError> {
        let name = required("name", req.name)?;
        req.metadata.validate(MetadataPolicy::capped())?;
        let now = Timestamp::now();
        let s = MemoryStoreEntity {
            archived_at: None,
            created_at: now,
            description: req.description.unwrap_or_default(),
            id: MemoryStoreId::generate(),
            metadata: req.metadata,
            name,
            tag: MemoryStoreTag::MemoryStore,
            updated_at: now,
        };
        self.store.insert_store(&ctx.workspace, s.clone()).await.map_err(map_store)?;
        Ok(s)
    }

    /// `GET /v1/memory_stores/{id}`.
    pub async fn get_store(&self, ctx: &AuthContext, raw_id: &str) -> Result<MemoryStoreEntity, ApiError> {
        let id = parse_store_id(raw_id)?;
        self.store.get_store(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/memory_stores`.
    pub async fn list_stores(
        &self,
        ctx: &AuthContext,
        page: &Pagination,
    ) -> Result<Vec<MemoryStoreEntity>, ApiError> {
        self.store.list_stores(&ctx.workspace, page).await.map_err(map_store)
    }

    /// `POST /v1/memory_stores/{id}/archive`.
    pub async fn archive_store(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<MemoryStoreEntity, ApiError> {
        let id = parse_store_id(raw_id)?;
        let mut s = self.store.get_store(&ctx.workspace, &id).await.map_err(map_store)?;
        if s.archived_at.is_none() {
            s.archived_at = Some(Timestamp::now());
            s.updated_at = Timestamp::now();
            self.store.replace_store(&ctx.workspace, s.clone()).await.map_err(map_store)?;
        }
        Ok(s)
    }

    /// `DELETE /v1/memory_stores/{id}` — live returned 200.
    pub async fn delete_store(&self, ctx: &AuthContext, raw_id: &str) -> Result<MemoryStoreEntity, ApiError> {
        let id = parse_store_id(raw_id)?;
        self.store.delete_store(&ctx.workspace, &id).await.map_err(map_store)
    }
}
