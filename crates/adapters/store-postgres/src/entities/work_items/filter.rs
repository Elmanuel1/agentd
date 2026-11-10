//! The filters a `work_items` statement scopes its rows by.

use std::time::Duration;

use ::work::store::{Reach, Scope, StoreError, WorkItem};
use diesel::pg::Pg;
use diesel::query_builder::BoxedSqlQuery;
use diesel::sql_types::{Nullable, Text};
use diesel_async::RunQueryDsl;
use shared::{Timestamp, WorkspaceId};

use super::row::{first, Row, COLS};
use super::PgWorkStore;
use crate::{diesel_be, PooledConnection};

pub(super) const KEY: &str = "workspace_id = $1 AND session_id = $2";
/// `$3` an environment id, `$4` a queue; both null reaches any item.
pub(super) const REACH: &str =
    "(($3::text IS NULL AND $4::text IS NULL) OR environment_id = $3 OR queue = $4)";

/// `$1` a workspace and `$2` an environment, or both null; `$3` a queue, or null.
pub(super) const SCOPE: &str =
    "($1::text IS NULL OR (workspace_id = $1 AND environment_id = $2)) AND ($3::text IS NULL OR queue = $3)";

pub(super) fn binds(scope: &Scope) -> (Option<&str>, Option<&str>, Option<&str>) {
    match scope {
        Scope::Environment { workspace, environment, queue } => {
            (Some(workspace.as_str()), Some(environment.as_str()), queue.as_deref())
        }
        Scope::Queue(queue) => (None, None, Some(queue.as_str())),
    }
}

/// `sql`, keyed by workspace and session (`$1`, `$2`) and reaching `reach` (`$3`, `$4`).
pub(super) fn keyed<'a>(
    sql: String,
    ws: &'a WorkspaceId,
    session: &'a str,
    reach: &'a Reach,
) -> BoxedSqlQuery<'a, Pg, diesel::query_builder::SqlQuery> {
    diesel::sql_query(sql)
        .into_boxed()
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(session)
        .bind::<Nullable<Text>, _>(reach.environment.as_deref())
        .bind::<Nullable<Text>, _>(reach.queue.as_deref())
}

impl PgWorkStore {
    pub(super) async fn reached(
        conn: &mut PooledConnection,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
    ) -> Result<Option<WorkItem>, StoreError> {
        let rows: Vec<Row> =
            keyed(format!("SELECT {COLS} FROM work_items WHERE {KEY} AND {REACH}"), ws, session, reach)
                .load(conn)
                .await
                .map_err(diesel_be)?;
        Ok(first(rows))
    }
}

/// `by` before `now`; a span reaching before the Unix epoch is the epoch, so
/// no item handed out since is taken for free.
pub(super) fn before(now: Timestamp, by: Duration) -> chrono::DateTime<chrono::Utc> {
    let at = chrono::Duration::from_std(by).ok().and_then(|by| now.as_datetime().checked_sub_signed(by));
    at.filter(|at| *at > chrono::DateTime::UNIX_EPOCH).unwrap_or(chrono::DateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reclaim_window_too_long_to_subtract_frees_nothing() {
        let now = Timestamp::now();
        assert_eq!(now.as_datetime() - before(now, Duration::from_secs(5)), chrono::Duration::seconds(5));
        for huge in [Duration::from_millis(u64::MAX), Duration::from_millis(1_000_000_000_000_000)] {
            assert_eq!(before(now, huge), chrono::DateTime::UNIX_EPOCH);
        }
    }
}
