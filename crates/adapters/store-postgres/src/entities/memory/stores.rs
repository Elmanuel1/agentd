use async_trait::async_trait;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use memory::store::{MemoryStorePersistence, StoreError};
use memory::types::{MemoryStoreEntity, MemoryStoreId};
use shared::{Pagination, WorkspaceId};

use super::PgMemoryStore;
use crate::schema::{memories, memory_stores, memory_versions};
use crate::{blob_be, diesel_be, parse_doc, pool_be, to_doc};

#[async_trait]
impl MemoryStorePersistence for PgMemoryStore {
    async fn insert_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(memory_stores::table)
            .values((
                memory_stores::workspace_id.eq(ws.as_str()),
                memory_stores::store_id.eq(s.id.as_str()),
                memory_stores::name.eq(&s.name),
                memory_stores::created_at.eq(s.created_at.as_datetime()),
                memory_stores::doc.eq(to_doc(&s)?),
            ))
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get_store(&self, ws: &WorkspaceId, id: &MemoryStoreId) -> Result<MemoryStoreEntity, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = memory_stores::table
            .filter(memory_stores::workspace_id.eq(ws.as_str()))
            .filter(memory_stores::store_id.eq(id.as_str()))
            .select(memory_stores::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::StoreNotFound)?)
    }

    async fn replace_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::update(
            memory_stores::table
                .filter(memory_stores::workspace_id.eq(ws.as_str()))
                .filter(memory_stores::store_id.eq(s.id.as_str())),
        )
        .set((memory_stores::name.eq(&s.name), memory_stores::doc.eq(to_doc(&s)?)))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::StoreNotFound);
        }
        Ok(())
    }

    async fn delete_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError> {
        let store = self.get_store(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let version_ids: Vec<String> = memory_versions::table
            .filter(memory_versions::workspace_id.eq(ws.as_str()))
            .filter(memory_versions::store_id.eq(id.as_str()))
            .filter(memory_versions::has_blob.eq(true))
            .select(memory_versions::version_id)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        diesel::delete(
            memory_versions::table
                .filter(memory_versions::workspace_id.eq(ws.as_str()))
                .filter(memory_versions::store_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        diesel::delete(
            memories::table
                .filter(memories::workspace_id.eq(ws.as_str()))
                .filter(memories::store_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        diesel::delete(
            memory_stores::table
                .filter(memory_stores::workspace_id.eq(ws.as_str()))
                .filter(memory_stores::store_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        for vid in version_ids {
            let key = format!("memory/{}/{}/{}", ws.as_str(), id.as_str(), vid);
            match self.blobs.delete(&key).await {
                Ok(()) | Err(blobs_s3::BlobError::NotFound(_)) => {}
                Err(e) => return Err(blob_be(e)),
            }
        }
        Ok(store)
    }

    async fn list_stores(
        &self,
        ws: &WorkspaceId,
        page: &Pagination,
    ) -> Result<Vec<MemoryStoreEntity>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = memory_stores::table
            .filter(memory_stores::workspace_id.eq(ws.as_str()))
            .order(memory_stores::created_at.desc())
            .select(memory_stores::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }
}
