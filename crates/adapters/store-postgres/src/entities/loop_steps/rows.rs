//! The rows the step queue's statements read.

use diesel::sql_types::{BigInt, Bool, Jsonb, Text};

#[derive(diesel::QueryableByName)]
pub(super) struct Due {
    #[diesel(sql_type = Text)]
    pub(super) workspace_id: String,
    #[diesel(sql_type = Text)]
    pub(super) session_id: String,
}

#[derive(diesel::QueryableByName)]
pub(super) struct Held {
    #[diesel(sql_type = BigInt)]
    pub(super) attempts: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) wakes: i64,
}

#[derive(diesel::QueryableByName)]
pub(super) struct Flag {
    #[diesel(sql_type = Bool)]
    pub(super) held: bool,
}

#[derive(diesel::QueryableByName)]
pub(super) struct Counts {
    #[diesel(sql_type = BigInt)]
    pub(super) due: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) leased: i64,
}

#[derive(diesel::QueryableByName)]
pub(super) struct Doc {
    #[diesel(sql_type = Jsonb)]
    pub(super) doc: serde_json::Value,
}
