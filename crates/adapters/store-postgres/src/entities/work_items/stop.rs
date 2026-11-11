//! Stopping a work item: on its own, or inside the transaction that deletes
//! its session.

use ::work::store::{Reach, WorkItem};
use diesel::sql_types::{Bool, Timestamptz};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::{Timestamp, WorkspaceId};

use super::filter::{keyed, KEY, REACH};
use super::row::{first, Row, COLS};

/// `stopped` when `force` or no worker holds the item, else `stopping`.
pub(crate) async fn stop(
    conn: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    session: &str,
    reach: &Reach,
    force: bool,
    now: Timestamp,
) -> Result<Option<WorkItem>, diesel::result::Error> {
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
    .load(conn)
    .await?;
    Ok(first(rows))
}
