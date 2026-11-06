use async_trait::async_trait;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use futures::StreamExt;
use memory::store::{MemoryStorePersistence, StoreError, StoreFilter};
use memory::types::{MemoryStoreEntity, MemoryStoreId};
use shared::{Pagination, WorkspaceId};

use super::PgMemoryStore;
use crate::schema::{memories, memory_stores, memory_versions};
use crate::{diesel_be, parse_doc, pool_be, to_doc};

#[async_trait]
impl MemoryStorePersistence for PgMemoryStore {
    async fn insert_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
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
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
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
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        // The rows go together; the blobs follow once the connection is back.
        let version_ids = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let version_ids: Vec<String> = memory_versions::table
                        .filter(memory_versions::workspace_id.eq(ws.as_str()))
                        .filter(memory_versions::store_id.eq(id.as_str()))
                        .filter(memory_versions::has_blob.eq(true))
                        .select(memory_versions::version_id)
                        .load(c)
                        .await?;
                    diesel::delete(
                        memory_versions::table
                            .filter(memory_versions::workspace_id.eq(ws.as_str()))
                            .filter(memory_versions::store_id.eq(id.as_str())),
                    )
                    .execute(c)
                    .await?;
                    diesel::delete(
                        memories::table
                            .filter(memories::workspace_id.eq(ws.as_str()))
                            .filter(memories::store_id.eq(id.as_str())),
                    )
                    .execute(c)
                    .await?;
                    diesel::delete(
                        memory_stores::table
                            .filter(memory_stores::workspace_id.eq(ws.as_str()))
                            .filter(memory_stores::store_id.eq(id.as_str())),
                    )
                    .execute(c)
                    .await?;
                    Ok(version_ids)
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?;
        drop(conn);
        // The store is gone either way, so a blob that will not delete is
        // logged as an orphan rather than failing the call.
        futures::stream::iter(version_ids)
            .for_each_concurrent(16, |vid| async move {
                let key = format!("memory/{}/{}/{}", ws.as_str(), id.as_str(), vid);
                match self.blobs.delete(&key).await {
                    Ok(()) | Err(shared::ports::blobs::BlobError::NotFound(_)) => {}
                    Err(e) => tracing::warn!(error = %e, key, "memory: orphaned a version blob"),
                }
            })
            .await;
        Ok(store)
    }

    async fn list_stores(
        &self,
        ws: &WorkspaceId,
        filter: &StoreFilter,
        page: &Pagination,
    ) -> Result<Vec<MemoryStoreEntity>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let mut q = memory_stores::table.filter(memory_stores::workspace_id.eq(ws.as_str())).into_boxed();
        if !filter.include_archived {
            q = q.filter(diesel::dsl::sql::<diesel::sql_types::Bool>("doc->>'archived_at' IS NULL"));
        }
        if let Some(t) = filter.created.gte {
            q = q.filter(memory_stores::created_at.ge(t.as_datetime()));
        }
        if let Some(t) = filter.created.lte {
            q = q.filter(memory_stores::created_at.le(t.as_datetime()));
        }
        let docs: Vec<serde_json::Value> = q
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
