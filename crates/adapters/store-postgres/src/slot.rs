//! The change feed's replication slot as Postgres reports it, and the WAL
//! written, for the `agentd.feed.*` metrics.

use diesel::sql_types::{BigInt, Bool, Nullable, Text};
use diesel_async::RunQueryDsl;
use shared::BackendError;

use crate::DieselPgPool;

/// What one read of a slot found.
#[derive(Debug, Clone, PartialEq, Eq, diesel::QueryableByName)]
pub struct Slot {
    /// The WAL the slot holds back; none when it is missing.
    #[diesel(sql_type = Nullable<BigInt>)]
    pub retained: Option<i64>,
    /// The WAL that may be written before Postgres invalidates the slot; none
    /// when `max_slot_wal_keep_size` is unlimited or the slot is lost or missing.
    #[diesel(sql_type = Nullable<BigInt>)]
    pub safe: Option<i64>,
    /// Invalidated (`wal_status = 'lost'`) or missing.
    #[diesel(sql_type = Bool)]
    pub lost: bool,
    /// The WAL written since Postgres's statistics were last reset.
    #[diesel(sql_type = BigInt)]
    pub wal_written: i64,
}

/// Read the slot `name` of `pool`'s database, which must be the primary.
pub async fn read_slot(pool: &DieselPgPool, name: &str) -> Result<Slot, BackendError> {
    let mut conn = pool.get().await.map_err(BackendError::new)?;
    diesel::sql_query(
        "SELECT pg_wal_lsn_diff(pg_current_wal_lsn(), s.restart_lsn)::bigint AS retained,
                s.safe_wal_size AS safe,
                (s.slot_name IS NULL OR s.wal_status = 'lost') AS lost,
                w.wal_bytes::bigint AS wal_written
           FROM pg_stat_wal w LEFT JOIN pg_replication_slots s ON s.slot_name = $1",
    )
    .bind::<Text, _>(name)
    .get_result(&mut *conn)
    .await
    .map_err(BackendError::new)
}
