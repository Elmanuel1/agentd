//! The reads of [`PgWorkStore`].

use std::time::Duration;

use ::work::store::{Counts, Reach, Scope, StoreError, WorkItem, WorkReads};
use async_trait::async_trait;
use diesel::sql_types::{BigInt, Nullable, Text, Timestamptz};
use diesel_async::RunQueryDsl;
use shared::{OrgId, Pagination, Timestamp, WorkspaceId};

use super::row::before;
use super::row::{first, item, CountsRow, OwnerRow, Row, COLS};
use super::PgWorkStore;
use crate::{diesel_be, pool_be};

/// `$1` a workspace and `$2` an environment, or both null; `$3` a queue, or null.
const SCOPE: &str =
    "($1::text IS NULL OR (workspace_id = $1 AND environment_id = $2)) AND ($3::text IS NULL OR queue = $3)";

fn binds(scope: &Scope) -> (Option<&str>, Option<&str>, Option<&str>) {
    match scope {
        Scope::Environment { workspace, environment, queue } => {
            (Some(workspace.as_str()), Some(environment.as_str()), queue.as_deref())
        }
        Scope::Queue(queue) => (None, None, Some(queue.as_str())),
    }
}

#[async_trait]
impl WorkReads for PgWorkStore {
    async fn get(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
    ) -> Result<Option<WorkItem>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        Self::reached(&mut conn, ws, session, reach).await
    }

    async fn find_on_queue(&self, queue: &str, session: &str) -> Result<Option<WorkItem>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<Row> =
            diesel::sql_query(format!("SELECT {COLS} FROM work_items WHERE queue = $1 AND session_id = $2"))
                .bind::<Text, _>(queue)
                .bind::<Text, _>(session)
                .load(&mut conn)
                .await
                .map_err(diesel_be)?;
        Ok(first(rows))
    }

    async fn list(&self, scope: &Scope, page: &Pagination) -> Result<Vec<WorkItem>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let (ws, env, queue) = binds(scope);
        let rows: Vec<Row> = diesel::sql_query(format!(
            "SELECT {COLS} FROM work_items WHERE {SCOPE} ORDER BY created_at DESC, session_id DESC LIMIT $4 OFFSET $5"
        ))
        .bind::<Nullable<Text>, _>(ws)
        .bind::<Nullable<Text>, _>(env)
        .bind::<Nullable<Text>, _>(queue)
        .bind::<BigInt, _>(page.fetch_limit())
        .bind::<BigInt, _>(page.sql_offset())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(rows.into_iter().map(item).collect())
    }

    async fn counts(&self, scope: &Scope, reclaim: Duration, now: Timestamp) -> Result<Counts, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // No poll hands out a hosted item, so none is counted.
        let (ws, env, queue) = binds(scope);
        let out: CountsRow = diesel::sql_query(format!(
            "SELECT count(*) FILTER (WHERE handed_out_at IS NULL OR handed_out_at < $4) AS depth, \
             count(*) FILTER (WHERE handed_out_at >= $4) AS pending, min(created_at) AS oldest \
             FROM work_items WHERE state = 'queued' AND queue <> 'hosted' AND {SCOPE}"
        ))
        .bind::<Nullable<Text>, _>(ws)
        .bind::<Nullable<Text>, _>(env)
        .bind::<Nullable<Text>, _>(queue)
        .bind::<Timestamptz, _>(before(now, reclaim))
        .get_result(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(Counts {
            depth: u64::try_from(out.depth).unwrap_or_default(),
            pending: u64::try_from(out.pending).unwrap_or_default(),
            oldest_queued_at: out.oldest.map(Timestamp::from_datetime),
        })
    }

    async fn runtime_owner(&self, environment: &str) -> Result<Option<(OrgId, WorkspaceId)>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<OwnerRow> = diesel::sql_query(
            "SELECT org_id, workspace_id FROM work_items WHERE environment_id = $1 AND queue = 'runtime' LIMIT 1",
        )
        .bind::<Text, _>(environment)
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(rows.into_iter().next().map(|r| (OrgId::new(r.org_id), WorkspaceId::new(r.workspace_id))))
    }
}
