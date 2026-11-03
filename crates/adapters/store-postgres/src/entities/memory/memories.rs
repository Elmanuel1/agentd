use async_trait::async_trait;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, TextExpressionMethods};
use diesel_async::RunQueryDsl;
use memory::store::{
    MemoryRecord, MemoryRecordPersistence, MemoryVersionPersistence, StoreError, VersionRecord,
};
use memory::types::{Memory, MemoryId, MemoryStoreId, MemoryVersionId};
use shared::WorkspaceId;

use super::heads::HeadWrite;
use super::PgMemoryStore;
use crate::schema::memories;
use crate::{diesel_be, parse_doc, pool_be};

#[async_trait]
impl MemoryRecordPersistence for PgMemoryStore {
    async fn insert_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError> {
        self.require_store(ws, store).await?;
        let write = HeadWrite {
            head: &record.head,
            deleted: false,
            versions: &record.chain,
            check: true,
            expected: None,
        };
        self.write_head(ws, store, &write).await
    }

    async fn get_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
    ) -> Result<MemoryRecord, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: Option<(serde_json::Value, bool)> = memories::table
            .filter(memories::workspace_id.eq(ws.as_str()))
            .filter(memories::store_id.eq(store.as_str()))
            .filter(memories::memory_id.eq(id.as_str()))
            .select((memories::doc, memories::deleted))
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        let Some((doc, deleted)) = row else {
            // A miss is store-not-found when the store itself is missing.
            self.require_store(ws, store).await?;
            return Err(StoreError::MemoryNotFound);
        };
        if deleted {
            return Err(StoreError::MemoryNotFound);
        }
        let head: Memory = parse_doc(doc)?;
        let (_, current) = self.get_version(ws, store, &head.memory_version_id).await?;
        Ok(MemoryRecord { head, deleted, chain: vec![current] })
    }

    async fn append_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        head: &Memory,
        deleted: bool,
        expected: &MemoryVersionId,
        version: &VersionRecord,
    ) -> Result<(), StoreError> {
        let versions = std::slice::from_ref(version);
        let write = HeadWrite { head, deleted, versions, check: !deleted, expected: Some(expected) };
        self.write_head(ws, store, &write).await
    }

    async fn list_memories(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path_prefix: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, StoreError> {
        self.require_store(ws, store).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let mut q = memories::table
            .filter(memories::workspace_id.eq(ws.as_str()))
            .filter(memories::store_id.eq(store.as_str()))
            .filter(memories::deleted.eq(false))
            .into_boxed();
        if let Some(p) = path_prefix {
            // Anchored prefix `LIKE 'p%'` — index-backed by the
            // `text_pattern_ops` index (V3). Escape the caller's `%`/`_`/`\` so
            // a path containing them can't act as a wildcard (ESCAPE '\').
            let escaped = p.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            q = q.filter(memories::path.like(format!("{escaped}%")));
        }
        let heads: Vec<serde_json::Value> =
            q.order(memories::path.asc()).select(memories::doc).load(&mut conn).await.map_err(diesel_be)?;
        // The listing projects the head only (basic view), so skip the
        // per-memory version-chain fetch entirely — no N+1.
        heads
            .into_iter()
            .map(|doc| Ok(MemoryRecord { head: parse_doc(doc)?, deleted: false, chain: Vec::new() }))
            .collect()
    }

    async fn find_by_path(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path: &str,
    ) -> Result<Option<MemoryId>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = memories::table
            .filter(memories::workspace_id.eq(ws.as_str()))
            .filter(memories::store_id.eq(store.as_str()))
            .filter(memories::path.eq(path))
            .filter(memories::deleted.eq(false))
            .select(memories::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        match doc {
            Some(d) => {
                let m: Memory = parse_doc(d)?;
                Ok(Some(m.id))
            }
            None => Ok(None),
        }
    }
}
