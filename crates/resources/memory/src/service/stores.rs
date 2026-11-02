use shared::limits::{required, validate_string_len};
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, Pagination, Timestamp};

use super::{map_store, parse_store_id, MemoriesService};
use crate::store::StoreFilter;
use crate::types::{
    CreateStoreRequest, MemoryStoreEntity, MemoryStoreId, MemoryStoreTag, UpdateStoreRequest,
};

impl MemoriesService {
    /// `POST /v1/memory_stores`.
    pub async fn create_store(
        &self,
        ctx: &AuthContext,
        req: CreateStoreRequest,
    ) -> Result<MemoryStoreEntity, ApiError> {
        let name = required("name", req.name)?;
        validate_string_len("name", &name, 1, 255)?;
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
        filter: &StoreFilter,
        page: &Pagination,
    ) -> Result<Vec<MemoryStoreEntity>, ApiError> {
        self.store.list_stores(&ctx.workspace, filter, page).await.map_err(map_store)
    }

    /// `POST /v1/memory_stores/{id}`: rename, describe or retag a live store.
    pub async fn update_store(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: UpdateStoreRequest,
    ) -> Result<MemoryStoreEntity, ApiError> {
        let id = parse_store_id(raw_id)?;
        let mut s = self.store.get_store(&ctx.workspace, &id).await.map_err(map_store)?;
        if s.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot update an archived memory store"));
        }
        if let Some(name) = req.name {
            validate_string_len("name", &name, 1, 255)?;
            s.name = name;
        }
        s.description = req.description.apply_to_option(Some(s.description)).unwrap_or_default();
        s.metadata = s.metadata.merged(req.metadata);
        s.metadata.validate(MetadataPolicy::capped())?;
        s.updated_at = Timestamp::now();
        self.store.replace_store(&ctx.workspace, s.clone()).await.map_err(map_store)?;
        Ok(s)
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
