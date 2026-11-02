use async_trait::async_trait;
use bytes::Bytes;
use diesel::sql_types::{BigInt, Nullable, Text, Timestamptz};
use diesel::upsert::excluded;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use memory::store::{
    MemoryRecord, MemoryRecordPersistence, MemoryVersionPersistence, StoreError, VersionFilter, VersionRecord,
};
use memory::types::{MemoryId, MemoryStoreId, MemoryVersion, MemoryVersionId};
use shared::{Pagination, WorkspaceId};

use super::PgMemoryStore;
use crate::schema::memory_versions;
use crate::{blob_be, diesel_be, parse_doc, pool_be, to_doc, DocRow};

impl PgMemoryStore {
    pub(super) async fn chain_full(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: &MemoryId,
    ) -> Result<Vec<VersionRecord>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<(serde_json::Value, bool)> = memory_versions::table
            .filter(memory_versions::workspace_id.eq(ws.as_str()))
            .filter(memory_versions::store_id.eq(store.as_str()))
            .filter(memory_versions::memory_id.eq(memory_id.as_str()))
            .order(memory_versions::seq.asc())
            .select((memory_versions::doc, memory_versions::has_blob))
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        // Fetch the chain's content blobs concurrently — they're independent
        // object-store reads, so a sequential loop would add latency linearly
        // on the hot `get_memory` path.
        let fetches = rows.into_iter().map(|(doc, has_blob)| async move {
            let version: MemoryVersion = parse_doc(doc)?;
            let blob = if has_blob {
                let bytes = self.blobs.get(&Self::blob_key(ws, store, &version.id)).await.map_err(blob_be)?;
                Some(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                None
            };
            Ok(VersionRecord { version, blob })
        });
        futures::future::try_join_all(fetches).await
    }

    pub(super) async fn upsert_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: &MemoryId,
        v: &VersionRecord,
    ) -> Result<(), StoreError> {
        let key = Self::blob_key(ws, store, &v.version.id);
        match &v.blob {
            Some(content) => {
                self.blobs.put(&key, Bytes::from(content.clone().into_bytes())).await.map_err(blob_be)?
            }
            None => match self.blobs.delete(&key).await {
                Ok(()) | Err(shared::ports::blobs::BlobError::NotFound(_)) => {}
                Err(e) => return Err(blob_be(e)),
            },
        }
        let mut meta = v.version.clone();
        meta.content = None;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(memory_versions::table)
            .values((
                memory_versions::workspace_id.eq(ws.as_str()),
                memory_versions::store_id.eq(store.as_str()),
                memory_versions::memory_id.eq(memory_id.as_str()),
                memory_versions::version_id.eq(v.version.id.as_str()),
                memory_versions::has_blob.eq(v.blob.is_some()),
                memory_versions::created_at.eq(v.version.created_at.as_datetime()),
                memory_versions::doc.eq(to_doc(&meta)?),
            ))
            .on_conflict((
                memory_versions::workspace_id,
                memory_versions::store_id,
                memory_versions::version_id,
            ))
            .do_update()
            .set((
                memory_versions::has_blob.eq(excluded(memory_versions::has_blob)),
                memory_versions::doc.eq(excluded(memory_versions::doc)),
            ))
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }
}

#[async_trait]
impl MemoryVersionPersistence for PgMemoryStore {
    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        filter: &VersionFilter,
        page: &Pagination,
    ) -> Result<Vec<VersionRecord>, StoreError> {
        self.require_store(ws, store).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let op = filter.operation.map(|o| to_doc::<_, StoreError>(&o)).transpose()?;
        let op = op.as_ref().and_then(serde_json::Value::as_str);
        let rows: Vec<DocRow> = diesel::sql_query(
            "SELECT doc FROM memory_versions WHERE workspace_id = $1 AND store_id = $2
               AND ($3::text IS NULL OR memory_id = $3)
               AND ($4::text IS NULL OR doc->'created_by'->>'api_key_id' = $4)
               AND ($5::text IS NULL OR doc->'created_by'->>'session_id' = $5)
               AND $6::text IS NULL
               AND ($7::text IS NULL OR doc->>'operation' = $7)
               AND ($8::timestamptz IS NULL OR created_at >= $8)
               AND ($9::timestamptz IS NULL OR created_at <= $9)
             ORDER BY seq DESC LIMIT $10 OFFSET $11",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(store.as_str())
        .bind::<Nullable<Text>, _>(filter.memory_id.as_deref())
        .bind::<Nullable<Text>, _>(filter.api_key_id.as_deref())
        .bind::<Nullable<Text>, _>(filter.session_id.as_deref())
        .bind::<Nullable<Text>, _>(filter.service_account_id.as_deref())
        .bind::<Nullable<Text>, _>(op)
        .bind::<Nullable<Timestamptz>, _>(filter.created.gte.map(|t| t.as_datetime()))
        .bind::<Nullable<Timestamptz>, _>(filter.created.lte.map(|t| t.as_datetime()))
        .bind::<BigInt, _>(page.fetch_limit())
        .bind::<BigInt, _>(page.sql_offset())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        rows.into_iter().map(|r| Ok(VersionRecord { version: parse_doc(r.doc)?, blob: None })).collect()
    }

    async fn get_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryVersionId,
    ) -> Result<(MemoryId, VersionRecord), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: Option<(serde_json::Value, bool)> = memory_versions::table
            .filter(memory_versions::workspace_id.eq(ws.as_str()))
            .filter(memory_versions::store_id.eq(store.as_str()))
            .filter(memory_versions::version_id.eq(id.as_str()))
            .select((memory_versions::doc, memory_versions::has_blob))
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        let (doc, has_blob) = row.ok_or(StoreError::VersionNotFound)?;
        let version: MemoryVersion = parse_doc(doc)?;
        let blob = if has_blob {
            let bytes = self.blobs.get(&Self::blob_key(ws, store, id)).await.map_err(blob_be)?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            None
        };
        let memory_id = version.memory_id.clone();
        Ok((memory_id, VersionRecord { version, blob }))
    }

    async fn rollback(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
        to: &MemoryVersionId,
    ) -> Result<MemoryRecord, StoreError> {
        let mut record = self.get_memory(ws, store, id).await?;
        let target =
            record.chain.iter().find(|v| &v.version.id == to).cloned().ok_or(StoreError::VersionNotFound)?;
        record.head.memory_version_id = target.version.id.clone();
        if let (Some(sha), Some(size), Some(path)) = (
            target.version.content_sha256.clone(),
            target.version.content_size_bytes,
            target.version.path.clone(),
        ) {
            record.head.content_sha256 = sha;
            record.head.content_size_bytes = size;
            record.head.path = path;
        }
        self.write_head(ws, store, &record, false).await?;
        Ok(record)
    }
}
