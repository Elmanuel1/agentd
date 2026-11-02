use async_trait::async_trait;
use diesel::sql_types::Text;
use diesel::upsert::excluded;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, TextExpressionMethods};
use diesel_async::RunQueryDsl;
use memory::store::{MemoryRecord, MemoryRecordPersistence, StoreError};
use memory::types::{Memory, MemoryId, MemoryStoreId};
use shared::WorkspaceId;

use super::PgMemoryStore;
use crate::schema::memories;
use crate::{diesel_be, parse_doc, pool_be, to_doc, DocRow};

impl PgMemoryStore {
    /// Live: a path conflicts with another live memory at the same path, or
    /// one that is its ancestor or descendant (`/notes` and `/notes/a.md`).
    async fn check_path(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: &MemoryRecord,
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<DocRow> = diesel::sql_query(
            "SELECT doc FROM memories WHERE workspace_id = $1 AND store_id = $2 AND NOT deleted AND memory_id <> $3
               AND (path = $4 OR starts_with($4, path || '/') OR starts_with(path, $4 || '/')) LIMIT 1",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(store.as_str())
        .bind::<Text, _>(record.head.id.as_str())
        .bind::<Text, _>(&record.head.path)
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        match rows.into_iter().next() {
            None => Ok(()),
            Some(row) => {
                let other: Memory = parse_doc(row.doc)?;
                Err(StoreError::PathConflict { path: other.path, memory_id: other.id })
            }
        }
    }

    pub(super) async fn upsert_head(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: &MemoryRecord,
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(memories::table)
            .values((
                memories::workspace_id.eq(ws.as_str()),
                memories::store_id.eq(store.as_str()),
                memories::memory_id.eq(record.head.id.as_str()),
                memories::path.eq(&record.head.path),
                memories::head_version_id.eq(record.head.memory_version_id.as_str()),
                memories::deleted.eq(record.deleted),
                memories::created_at.eq(record.head.created_at.as_datetime()),
                memories::doc.eq(to_doc(&record.head)?),
            ))
            .on_conflict((memories::workspace_id, memories::store_id, memories::memory_id))
            .do_update()
            .set((
                memories::path.eq(excluded(memories::path)),
                memories::head_version_id.eq(excluded(memories::head_version_id)),
                memories::deleted.eq(excluded(memories::deleted)),
                memories::doc.eq(excluded(memories::doc)),
            ))
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }
}

#[async_trait]
impl MemoryRecordPersistence for PgMemoryStore {
    async fn insert_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError> {
        self.require_store(ws, store).await?;
        self.check_path(ws, store, &record).await?;
        self.upsert_head(ws, store, &record).await?;
        for v in &record.chain {
            self.upsert_version(ws, store, &record.head.id, v).await?;
        }
        Ok(())
    }

    async fn get_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
    ) -> Result<MemoryRecord, StoreError> {
        self.require_store(ws, store).await?;
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
        let (doc, deleted) = row.ok_or(StoreError::MemoryNotFound)?;
        if deleted {
            return Err(StoreError::MemoryNotFound);
        }
        let head: Memory = parse_doc(doc)?;
        let chain = self.chain_full(ws, store, id).await?;
        Ok(MemoryRecord { head, deleted, chain })
    }

    async fn replace_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError> {
        self.require_store(ws, store).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = memories::table
            .filter(memories::workspace_id.eq(ws.as_str()))
            .filter(memories::store_id.eq(store.as_str()))
            .filter(memories::memory_id.eq(record.head.id.as_str()))
            .select(memories::memory_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        if exists.is_none() {
            return Err(StoreError::MemoryNotFound);
        }
        if !record.deleted {
            self.check_path(ws, store, &record).await?;
        }
        self.upsert_head(ws, store, &record).await?;
        for v in &record.chain {
            self.upsert_version(ws, store, &record.head.id, v).await?;
        }
        Ok(())
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
