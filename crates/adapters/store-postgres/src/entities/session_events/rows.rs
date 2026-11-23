//! The rows the event log's stream reads return.

use diesel::sql_types::{Array, BigInt, Bool, Jsonb, Nullable};

#[derive(diesel::QueryableByName)]
pub(super) struct StateRow {
    #[diesel(sql_type = Bool)]
    pub(super) archived: bool,
    #[diesel(sql_type = BigInt)]
    pub(super) unsettled: i64,
}

#[derive(diesel::QueryableByName)]
pub(super) struct HeadRow {
    #[diesel(sql_type = Nullable<BigInt>)]
    pub(super) xid: Option<i64>,
    #[diesel(sql_type = Nullable<BigInt>)]
    pub(super) seq: Option<i64>,
    #[diesel(sql_type = Array<BigInt>)]
    pub(super) queued: Vec<i64>,
}

#[derive(diesel::QueryableByName)]
pub(super) struct TailRow {
    #[diesel(sql_type = BigInt)]
    pub(super) xid: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) seq: i64,
    #[diesel(sql_type = Jsonb)]
    pub(super) doc: serde_json::Value,
}

#[derive(diesel::QueryableByName)]
pub(super) struct SeqDoc {
    #[diesel(sql_type = BigInt)]
    pub(super) seq: i64,
    #[diesel(sql_type = Jsonb)]
    pub(super) doc: serde_json::Value,
}

#[derive(diesel::QueryableByName)]
pub(super) struct CallsRow {
    #[diesel(sql_type = BigInt)]
    pub(super) cache_read: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) input: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) output: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) unpriced: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) cost: i64,
}
