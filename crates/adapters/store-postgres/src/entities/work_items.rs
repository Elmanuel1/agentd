//! `WorkStore` on Postgres: one `work_items` row per session. A claim takes
//! the oldest queued row of a queue with `FOR UPDATE SKIP LOCKED`; every
//! other write is one guarded `UPDATE … RETURNING`, read again only when it
//! misses, to tell the caller what the item holds.

mod reads;
mod row;

use std::collections::BTreeMap;
use std::time::Duration;

use ::work::store::{Beat, Reach, StoreError, WorkItem, WorkStore};
use async_trait::async_trait;
use diesel::sql_types::{Array, Bool, Jsonb, Nullable, Text, Timestamptz};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::{Timestamp, WorkspaceId};

use self::row::{before, first, insert, keyed, Row, COLS, KEY, REACH};
use crate::{diesel_be, pool_be, PgBackend, PooledConnection};

#[derive(Debug, Clone)]
pub struct PgWorkStore {
    backend: PgBackend,
}

impl PgWorkStore {
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

impl PgWorkStore {
    async fn write(&self) -> Result<PooledConnection, StoreError> {
        self.backend.diesel.write().await.map_err(pool_be)
    }
}

#[async_trait]
impl WorkStore for PgWorkStore {
    async fn queue(&self, i: &WorkItem) -> Result<(), StoreError> {
        insert(i).execute(&mut self.write().await?).await.map(|_| ()).map_err(diesel_be)
    }

    async fn claim(
        &self,
        queue: &str,
        workspace: Option<&WorkspaceId>,
        reclaim: Duration,
        now: Timestamp,
    ) -> Result<Option<WorkItem>, StoreError> {
        let mut conn = self.write().await?;
        let (queue, free_before) = (queue.to_owned(), before(now, reclaim));
        let workspace = workspace.map(|w| w.as_str().to_owned());
        let rows: Vec<Row> = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let rows: Vec<Row> = diesel::sql_query(format!(
                        "SELECT {COLS} FROM work_items WHERE queue = $1 AND state = 'queued' \
                         AND (handed_out_at IS NULL OR handed_out_at < $2) \
                         AND ($3::text IS NULL OR workspace_id = $3) \
                         ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED"
                    ))
                    .bind::<Text, _>(&queue)
                    .bind::<Timestamptz, _>(free_before)
                    .bind::<Nullable<Text>, _>(workspace.as_deref())
                    .load(c)
                    .await?;
                    if let Some(r) = <[Row]>::first(&rows) {
                        diesel::sql_query(format!("UPDATE work_items SET handed_out_at = $3 WHERE {KEY}"))
                            .bind::<Text, _>(&r.workspace_id)
                            .bind::<Text, _>(&r.session_id)
                            .bind::<Timestamptz, _>(now.as_datetime())
                            .execute(c)
                            .await?;
                    }
                    Ok(rows)
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?;
        Ok(first(rows).map(|i| WorkItem { handed_out_at: Some(now), ..i }))
    }

    async fn update_metadata(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        set: &BTreeMap<String, String>,
        remove: &[String],
    ) -> Result<Option<WorkItem>, StoreError> {
        let rows: Vec<Row> = keyed(
            format!(
            "UPDATE work_items SET metadata = (metadata || $5) - $6 WHERE {KEY} AND {REACH} RETURNING {COLS}"
        ),
            ws,
            session,
            reach,
        )
        .bind::<Jsonb, _>(serde_json::to_value(set).unwrap_or_default())
        .bind::<Array<Text>, _>(remove)
        .load(&mut self.write().await?)
        .await
        .map_err(diesel_be)?;
        Ok(first(rows))
    }

    async fn ack(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<Option<WorkItem>, StoreError> {
        let mut conn = self.write().await?;
        let rows: Vec<Row> = keyed(
            format!(
                "UPDATE work_items SET state = 'starting', acknowledged_at = $5, lease_until = $6 \
             WHERE {KEY} AND {REACH} AND state = 'queued' RETURNING {COLS}"
            ),
            ws,
            session,
            reach,
        )
        .bind::<Timestamptz, _>(now.as_datetime())
        .bind::<Timestamptz, _>(lease_until.as_datetime())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        match first(rows) {
            Some(i) => Ok(Some(i)),
            None => Self::reached(&mut conn, ws, session, reach).await,
        }
    }

    async fn heartbeat(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        beat: &Beat,
    ) -> Result<Option<(WorkItem, bool)>, StoreError> {
        let mut conn = self.write().await?;
        let rows: Vec<Row> = keyed(
            format!(
                "UPDATE work_items SET state = 'active', latest_heartbeat_at = $5, last_heartbeat = $6, \
             lease_until = $7 WHERE {KEY} AND {REACH} AND state NOT IN ('stopping', 'stopped') \
             AND ($8::text IS NULL OR coalesce(last_heartbeat, 'NO_HEARTBEAT') = $8) RETURNING {COLS}"
            ),
            ws,
            session,
            reach,
        )
        .bind::<Timestamptz, _>(beat.at.as_datetime())
        .bind::<Text, _>(&beat.stamp)
        .bind::<Timestamptz, _>(beat.lease_until.as_datetime())
        .bind::<Nullable<Text>, _>(beat.expected.as_deref())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        if let Some(i) = first(rows) {
            return Ok(Some((i, true)));
        }
        Ok(Self::reached(&mut conn, ws, session, reach).await?.map(|i| (i, false)))
    }

    async fn stop(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        force: bool,
        now: Timestamp,
    ) -> Result<Option<WorkItem>, StoreError> {
        // No worker holds a queued item, nor one whose lease ran out.
        let free = "state = 'queued' OR lease_until < $6";
        let rows: Vec<Row> = keyed(
            format!(
                "UPDATE work_items SET stop_requested_at = coalesce(stop_requested_at, $6), \
             stopped_at = CASE WHEN state = 'stopped' THEN stopped_at WHEN $5 OR {free} THEN $6 END, \
             state = CASE WHEN state = 'stopped' OR $5 OR {free} THEN 'stopped' ELSE 'stopping' END \
             WHERE {KEY} AND {REACH} RETURNING {COLS}"
            ),
            ws,
            session,
            reach,
        )
        .bind::<Bool, _>(force)
        .bind::<Timestamptz, _>(now.as_datetime())
        .load(&mut self.write().await?)
        .await
        .map_err(diesel_be)?;
        Ok(first(rows))
    }
}
