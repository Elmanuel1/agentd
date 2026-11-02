//! Writing a memory's head row and its versions, with the live path-overlap
//! rule and the expected-head guard.

use diesel::sql_types::{Array, Text};
use diesel::ExpressionMethods;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use memory::store::{StoreError, VersionRecord};
use memory::types::{Memory, MemoryStoreId, MemoryVersionId};
use shared::WorkspaceId;

use super::versions::version_row;
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
    /// blobs. With `check`, the overlap check runs first in that transaction
    /// under a per-store advisory lock: only the exact path has a unique
    /// index, so two writers of `/x` and `/x/y.md` would otherwise both pass.
    pub(super) async fn write_head(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        w: &HeadWrite<'_>,
    ) -> Result<(), StoreError> {
        for v in w.versions {
            self.put_blob(ws, store, v).await?;
        }
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc = to_doc(w.head)?;
        let outcome = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    if w.check {
                        if let Some(row) = overlap(c, ws, store, w.head).await? {
                            return Ok(Outcome::Conflict(row.doc));
                        }
                    }
                    if !head_row(c, ws, store, w, doc).await? {
                        return Ok(Outcome::Moved);
                    }
                    for v in w.versions {
                        version_row(c, ws, store, &w.head.id, v).await?;
                    }
                    Ok(Outcome::Written)
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?;
        match outcome {
            Outcome::Written => Ok(()),
            Outcome::Moved => Err(StoreError::HeadMoved),
            Outcome::Conflict(doc) => {
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

/// Insert or update the head row; with `expected`, only while the stored
/// head is still that version. False when the head had moved.
async fn head_row(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    store: &MemoryStoreId,
    w: &HeadWrite<'_>,
    doc: serde_json::Value,
) -> Result<bool, diesel::result::Error> {
    let (head, deleted) = (w.head, w.deleted);
    let changes = (
        memories::path.eq(&head.path),
        memories::head_version_id.eq(head.memory_version_id.as_str()),
        memories::deleted.eq(deleted),
        memories::doc.eq(doc),
    );
    let rows = match w.expected {
        Some(expected) => {
            diesel::update(memories::table)
                .filter(memories::workspace_id.eq(ws.as_str()))
                .filter(memories::store_id.eq(store.as_str()))
                .filter(memories::memory_id.eq(head.id.as_str()))
                .filter(memories::head_version_id.eq(expected.as_str()))
                .set(changes)
                .execute(c)
                .await?
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
                .await?
        }
    };
    Ok(rows == 1)
}
