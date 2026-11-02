use async_trait::async_trait;
use bytes::Bytes;
use diesel::sql_types::{BigInt, Nullable, Text, Timestamptz};
use diesel::upsert::excluded;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use memory::store::{
    MemoryRecord, MemoryRecordPersistence, MemoryVersionPersistence, StoreError, VersionFilter, VersionRecord,
};
use memory::types::{MemoryId, MemoryStoreId, MemoryVersion, MemoryVersionId};
use shared::{Pagination, WorkspaceId};

use super::heads::HeadWrite;
use super::PgMemoryStore;
use crate::schema::memory_versions;
use crate::{blob_be, diesel_be, parse_doc, pool_be, to_doc, DocRow};

impl PgMemoryStore {
    /// Put a version's content blob, or remove it when the version has none.
    pub(super) async fn put_blob(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        v: &VersionRecord,
    ) -> Result<(), StoreError> {
        let key = Self::blob_key(ws, store, &v.version.id);
        match &v.blob {
            Some(content) => {
                self.blobs.put(&key, Bytes::from(content.clone().into_bytes())).await.map_err(blob_be)
            }
            None => match self.blobs.delete(&key).await {
                Ok(()) | Err(shared::ports::blobs::BlobError::NotFound(_)) => Ok(()),
                Err(e) => Err(blob_be(e)),
            },
        }
    }
}

/// Upsert a version's row; its content lives in the blob.
pub(super) async fn version_row(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    store: &MemoryStoreId,
    memory_id: &MemoryId,
    v: &VersionRecord,
) -> Result<(), diesel::result::Error> {
    let mut meta = v.version.clone();
    meta.content = None;
    let doc = serde_json::to_value(&meta).map_err(|e| diesel::result::Error::SerializationError(e.into()))?;
    diesel::insert_into(memory_versions::table)
        .values((
            memory_versions::workspace_id.eq(ws.as_str()),
            memory_versions::store_id.eq(store.as_str()),
            memory_versions::memory_id.eq(memory_id.as_str()),
            memory_versions::version_id.eq(v.version.id.as_str()),
            memory_versions::has_blob.eq(v.blob.is_some()),
            memory_versions::created_at.eq(v.version.created_at.as_datetime()),
            memory_versions::doc.eq(doc),
        ))
        .on_conflict((memory_versions::workspace_id, memory_versions::store_id, memory_versions::version_id))
        .do_update()
        .set((
            memory_versions::has_blob.eq(excluded(memory_versions::has_blob)),
            memory_versions::doc.eq(excluded(memory_versions::doc)),
        ))
        .execute(c)
        .await?;
    Ok(())
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

    async fn replace_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: &MemoryId,
        version: &VersionRecord,
    ) -> Result<(), StoreError> {
        self.put_blob(ws, store, version).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        version_row(&mut conn, ws, store, memory_id, version).await.map_err(diesel_be)
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
        let record = self.get_memory(ws, store, id).await?;
        let (owner, target) = self.get_version(ws, store, to).await?;
        if &owner != id {
            return Err(StoreError::VersionNotFound);
        }
        let mut head = record.head;
        let expected = std::mem::replace(&mut head.memory_version_id, target.version.id.clone());
        if let (Some(sha), Some(size), Some(path)) = (
            target.version.content_sha256.clone(),
            target.version.content_size_bytes,
            target.version.path.clone(),
        ) {
            head.content_sha256 = sha;
            head.content_size_bytes = size;
            head.path = path;
        }
        let write =
            HeadWrite { head: &head, deleted: false, versions: &[], check: false, expected: Some(&expected) };
        self.write_head(ws, store, &write).await?;
        Ok(MemoryRecord { head, deleted: false, chain: vec![target] })
    }
}
