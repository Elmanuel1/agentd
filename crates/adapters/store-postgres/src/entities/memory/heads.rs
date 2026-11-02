//! Writing a memory's head row and its versions, with the live path-overlap
//! rule and the expected-head guard.

use diesel::sql_types::{Array, Text};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use memory::store::{StoreError, VersionRecord};
use memory::types::{Memory, MemoryId, MemoryStoreId, MemoryVersionId};
use shared::WorkspaceId;

use super::versions::{version_doc, version_row};
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

/// One head write: the head row, the versions written with it, whether the
/// path-overlap rule applies, and the head it must still find (none for an
/// insert).
pub(super) struct HeadWrite<'a> {
    pub head: &'a Memory,
    pub deleted: bool,
    pub versions: &'a [VersionRecord],
    pub check: bool,
    pub expected: Option<&'a MemoryVersionId>,
}

enum Outcome {
    Written,
    Conflict(serde_json::Value),
    Moved,
}

impl PgMemoryStore {
    /// Write the head and its versions' rows in one transaction, after their
    /// blobs (which are removed again when the write does not land). An
    /// update locks its memory's row and must find the head still at
    /// `expected`. The overlap check runs only when the path is new to the
    /// row, under a per-store advisory lock: only the exact path has a unique
    /// index, so two writers of `/x` and `/x/y.md` would otherwise both pass.
    pub(super) async fn write_head(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        w: &HeadWrite<'_>,
    ) -> Result<(), StoreError> {
        let docs = w.versions.iter().map(version_doc).collect::<Result<Vec<_>, StoreError>>()?;
        let head_doc = to_doc(w.head)?;
        for v in w.versions.iter().filter(|v| v.blob.is_some()) {
            self.put_blob(ws, store, v).await?;
        }
        let outcome = self.transact(ws, store, w, head_doc, docs).await;
        if !matches!(outcome, Ok(Outcome::Written)) {
            self.drop_blobs(ws, store, w.versions).await;
        }
        match outcome? {
            Outcome::Written => Ok(()),
            Outcome::Moved => Err(StoreError::HeadMoved),
            Outcome::Conflict(doc) => {
                let other: Memory = parse_doc(doc)?;
                Err(StoreError::PathConflict { path: other.path, memory_id: other.id })
            }
        }
    }

    async fn transact(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        w: &HeadWrite<'_>,
        head_doc: serde_json::Value,
        docs: Vec<serde_json::Value>,
    ) -> Result<Outcome, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                let stored_path = match w.expected {
                    Some(expected) => match locked_path(c, ws, store, &w.head.id, expected).await? {
                        Some(path) => Some(path),
                        None => return Ok(Outcome::Moved),
                    },
                    None => None,
                };
                if w.check && stored_path.as_deref() != Some(w.head.path.as_str()) {
                    if let Some(row) = overlap(c, ws, store, w.head).await? {
                        return Ok(Outcome::Conflict(row.doc));
                    }
                }
                head_row(c, ws, store, w, head_doc).await?;
                for (v, doc) in w.versions.iter().zip(docs) {
                    version_row(c, ws, store, &w.head.id, v, doc).await?;
                }
                Ok(Outcome::Written)
            }
            .scope_boxed()
        })
        .await
        .map_err(diesel_be)
    }

    async fn drop_blobs(&self, ws: &WorkspaceId, store: &MemoryStoreId, versions: &[VersionRecord]) {
        for v in versions.iter().filter(|v| v.blob.is_some()) {
            let key = Self::blob_key(ws, store, &v.version.id);
            if let Err(e) = self.blobs.delete(&key).await {
                tracing::warn!(error = %e, key, "memory: could not remove the blob of a version that was not written");
            }
        }
    }
}

/// The memory's stored path, locking its row, while it is live and its head
/// is `expected`: a write that read the head before a delete finds nothing,
/// so it cannot bring the memory back.
async fn locked_path(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    store: &MemoryStoreId,
    id: &MemoryId,
    expected: &MemoryVersionId,
) -> Result<Option<String>, diesel::result::Error> {
    memories::table
        .filter(memories::workspace_id.eq(ws.as_str()))
        .filter(memories::store_id.eq(store.as_str()))
        .filter(memories::memory_id.eq(id.as_str()))
        .filter(memories::head_version_id.eq(expected.as_str()))
        .filter(memories::deleted.eq(false))
        .select(memories::path)
        .for_update()
        .first(c)
        .await
        .optional()
}

async fn overlap(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    store: &MemoryStoreId,
    head: &Memory,
) -> Result<Option<DocRow>, diesel::result::Error> {
    diesel::sql_query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind::<Text, _>(format!("memory:{}:{}", ws.as_str(), store.as_str()))
        .execute(c)
        .await?;
    let path = &head.path;
    let mut exact: Vec<String> =
        path.match_indices('/').filter(|(i, _)| *i > 0).map(|(i, _)| path[..i].to_owned()).collect();
    exact.push(path.clone());
    let rows: Vec<DocRow> = diesel::sql_query(OVERLAP)
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(store.as_str())
        .bind::<Text, _>(head.id.as_str())
        .bind::<Array<Text>, _>(&exact)
        .bind::<Text, _>(format!("{path}/"))
        // `0` is the byte after `/`, so the range is every `{path}/…`.
        .bind::<Text, _>(format!("{path}0"))
        .load(c)
        .await?;
    Ok(rows.into_iter().next())
}

/// Insert the head row, or update the row `locked_path` locked.
async fn head_row(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    store: &MemoryStoreId,
    w: &HeadWrite<'_>,
    doc: serde_json::Value,
) -> Result<(), diesel::result::Error> {
    let (head, deleted) = (w.head, w.deleted);
    let changes = (
        memories::path.eq(&head.path),
        memories::head_version_id.eq(head.memory_version_id.as_str()),
        memories::deleted.eq(deleted),
        memories::doc.eq(doc),
    );
    match w.expected {
        Some(expected) => {
            diesel::update(memories::table)
                .filter(memories::workspace_id.eq(ws.as_str()))
                .filter(memories::store_id.eq(store.as_str()))
                .filter(memories::memory_id.eq(head.id.as_str()))
                .filter(memories::head_version_id.eq(expected.as_str()))
                .set(changes)
                .execute(c)
                .await?;
        }
        None => {
            diesel::insert_into(memories::table)
                .values((
                    memories::workspace_id.eq(ws.as_str()),
                    memories::store_id.eq(store.as_str()),
                    memories::memory_id.eq(head.id.as_str()),
                    memories::created_at.eq(head.created_at.as_datetime()),
                    changes,
                ))
                .execute(c)
                .await?;
        }
    }
    Ok(())
}
