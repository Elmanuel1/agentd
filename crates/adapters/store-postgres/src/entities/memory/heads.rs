//! Writing a memory's head row, with the live path-overlap rule.

use diesel::sql_types::{Array, Text};
use diesel::upsert::excluded;
use diesel::ExpressionMethods;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use memory::store::{MemoryRecord, StoreError};
use memory::types::{Memory, MemoryStoreId};
use shared::WorkspaceId;

use super::PgMemoryStore;
use crate::schema::memories;
use crate::{diesel_be, parse_doc, pool_be, to_doc, DocRow};

/// Live: a path conflicts with another live memory at the same path, or one
/// that is its ancestor or descendant (`/notes` and `/notes/a.md`). Ancestors
/// and the path itself match by equality, descendants by a byte range, so
/// both the unique path index and the `text_pattern_ops` index serve it.
const OVERLAP: &str = "SELECT doc FROM memories
    WHERE workspace_id = $1 AND store_id = $2 AND NOT deleted AND memory_id <> $3
      AND (path = ANY($4) OR (path ~>=~ $5 AND path ~<~ $6)) LIMIT 1";

impl PgMemoryStore {
    /// Upsert the head. With `check`, the overlap check and the write run in
    /// one transaction under a per-store advisory lock: only the exact path
    /// has a unique index, so two writers of `/x` and `/x/y.md` would
    /// otherwise both pass the check.
    pub(super) async fn write_head(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: &MemoryRecord,
        check: bool,
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let head = to_doc(&record.head)?;
        let conflict = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    if check {
                        if let Some(row) = overlap(c, ws, store, record).await? {
                            return Ok(Some(row.doc));
                        }
                    }
                    upsert(c, ws, store, record, head).await?;
                    Ok(None)
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?;
        match conflict {
            None => Ok(()),
            Some(doc) => {
                let other: Memory = parse_doc(doc)?;
                Err(StoreError::PathConflict { path: other.path, memory_id: other.id })
            }
        }
    }
}

async fn overlap(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    store: &MemoryStoreId,
    record: &MemoryRecord,
) -> Result<Option<DocRow>, diesel::result::Error> {
    diesel::sql_query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind::<Text, _>(format!("memory:{}:{}", ws.as_str(), store.as_str()))
        .execute(c)
        .await?;
    let path = &record.head.path;
    let mut exact: Vec<String> =
        path.match_indices('/').filter(|(i, _)| *i > 0).map(|(i, _)| path[..i].to_owned()).collect();
    exact.push(path.clone());
    let rows: Vec<DocRow> = diesel::sql_query(OVERLAP)
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(store.as_str())
        .bind::<Text, _>(record.head.id.as_str())
        .bind::<Array<Text>, _>(&exact)
        .bind::<Text, _>(format!("{path}/"))
        // `0` is the byte after `/`, so the range is every `{path}/…`.
        .bind::<Text, _>(format!("{path}0"))
        .load(c)
        .await?;
    Ok(rows.into_iter().next())
}

async fn upsert(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    store: &MemoryStoreId,
    record: &MemoryRecord,
    doc: serde_json::Value,
) -> Result<(), diesel::result::Error> {
    diesel::insert_into(memories::table)
        .values((
            memories::workspace_id.eq(ws.as_str()),
            memories::store_id.eq(store.as_str()),
            memories::memory_id.eq(record.head.id.as_str()),
            memories::path.eq(&record.head.path),
            memories::head_version_id.eq(record.head.memory_version_id.as_str()),
            memories::deleted.eq(record.deleted),
            memories::created_at.eq(record.head.created_at.as_datetime()),
            memories::doc.eq(doc),
        ))
        .on_conflict((memories::workspace_id, memories::store_id, memories::memory_id))
        .do_update()
        .set((
            memories::path.eq(excluded(memories::path)),
            memories::head_version_id.eq(excluded(memories::head_version_id)),
            memories::deleted.eq(excluded(memories::deleted)),
            memories::doc.eq(excluded(memories::doc)),
        ))
        .execute(c)
        .await?;
    Ok(())
}
