//! A `work_items` row and the item it serves.

use std::collections::BTreeMap;

use ::work::store::WorkItem;
use ::work::types::{Work, WorkData, WorkDataTag, WorkState, WorkTag};
use chrono::{DateTime, Utc};
use diesel::pg::Pg;
use diesel::query_builder::BoxedSqlQuery;
use diesel::sql_types::{BigInt, Jsonb, Nullable, Text, Timestamptz};

use shared::{OrgId, Timestamp, WorkspaceId};

pub(super) const COLS: &str =
    "workspace_id, session_id, org_id, environment_id, queue, state, metadata, created_at, \
    acknowledged_at, started_at, latest_heartbeat_at, last_heartbeat, lease_until, handed_out_at, \
    stop_requested_at, stopped_at";

#[derive(diesel::QueryableByName)]
pub(super) struct Row {
    #[diesel(sql_type = Text)]
    pub(super) workspace_id: String,
    #[diesel(sql_type = Text)]
    pub(super) session_id: String,
    #[diesel(sql_type = Text)]
    pub(super) org_id: String,
    #[diesel(sql_type = Text)]
    pub(super) environment_id: String,
    #[diesel(sql_type = Text)]
    pub(super) queue: String,
    #[diesel(sql_type = Text)]
    pub(super) state: String,
    #[diesel(sql_type = Jsonb)]
    pub(super) metadata: serde_json::Value,
    #[diesel(sql_type = Timestamptz)]
    pub(super) created_at: DateTime<Utc>,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    pub(super) acknowledged_at: Option<DateTime<Utc>>,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    pub(super) started_at: Option<DateTime<Utc>>,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    pub(super) latest_heartbeat_at: Option<DateTime<Utc>>,
    #[diesel(sql_type = Nullable<Text>)]
    pub(super) last_heartbeat: Option<String>,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    pub(super) lease_until: Option<DateTime<Utc>>,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    pub(super) handed_out_at: Option<DateTime<Utc>>,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    pub(super) stop_requested_at: Option<DateTime<Utc>>,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    pub(super) stopped_at: Option<DateTime<Utc>>,
}

#[derive(diesel::QueryableByName)]
pub(super) struct CountsRow {
    #[diesel(sql_type = BigInt)]
    pub(super) depth: i64,
    #[diesel(sql_type = BigInt)]
    pub(super) pending: i64,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    pub(super) oldest: Option<DateTime<Utc>>,
}

#[derive(diesel::QueryableByName)]
pub(super) struct OwnerRow {
    #[diesel(sql_type = Text)]
    pub(super) org_id: String,
    #[diesel(sql_type = Text)]
    pub(super) workspace_id: String,
}

fn ts(at: Option<DateTime<Utc>>) -> Option<Timestamp> {
    at.map(Timestamp::from_datetime)
}

pub(super) fn item(r: Row) -> WorkItem {
    let metadata: BTreeMap<String, String> = serde_json::from_value(r.metadata).unwrap_or_default();
    let work = Work {
        id: r.session_id.clone(),
        acknowledged_at: ts(r.acknowledged_at),
        actor: None,
        created_at: Timestamp::from_datetime(r.created_at),
        data: WorkData { id: r.session_id, tag: WorkDataTag::Session },
        environment_id: r.environment_id,
        latest_heartbeat_at: ts(r.latest_heartbeat_at),
        metadata,
        secret: None,
        started_at: ts(r.started_at),
        state: WorkState::parse(&r.state).unwrap_or(WorkState::Stopped),
        stop_requested_at: ts(r.stop_requested_at),
        stopped_at: ts(r.stopped_at),
        tag: WorkTag::Work,
    };
    WorkItem {
        workspace: WorkspaceId::new(r.workspace_id),
        org: OrgId::new(r.org_id),
        queue: r.queue,
        work,
        last_heartbeat: r.last_heartbeat,
        lease_until: ts(r.lease_until),
        handed_out_at: ts(r.handed_out_at),
    }
    .served(Timestamp::now())
}

pub(super) fn first(rows: Vec<Row>) -> Option<WorkItem> {
    rows.into_iter().next().map(item)
}

/// The `INSERT` of `i`, which queues a `stopped` item of its session again
/// (one whose `stopping` lease ran out is `stopped`); any other is untouched.
pub(super) fn insert(i: &WorkItem) -> BoxedSqlQuery<'_, Pg, diesel::query_builder::SqlQuery> {
    let w = &i.work;
    let metadata = serde_json::to_value(&w.metadata).unwrap_or_default();
    diesel::sql_query(format!(
        "INSERT INTO work_items ({COLS}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
             $14, $15, $16) ON CONFLICT (workspace_id, session_id) DO UPDATE SET queue = EXCLUDED.queue, \
             environment_id = EXCLUDED.environment_id, state = 'queued', started_at = EXCLUDED.started_at, \
             acknowledged_at = NULL, latest_heartbeat_at = NULL, last_heartbeat = NULL, lease_until = NULL, \
             handed_out_at = NULL, stop_requested_at = NULL, stopped_at = NULL \
             WHERE work_items.state = 'stopped' \
             OR (work_items.state = 'stopping' AND work_items.lease_until < EXCLUDED.started_at)"
    ))
    .into_boxed()
    .bind::<Text, _>(i.workspace.as_str())
    .bind::<Text, _>(&w.id)
    .bind::<Text, _>(i.org.as_str())
    .bind::<Text, _>(&w.environment_id)
    .bind::<Text, _>(&i.queue)
    .bind::<Text, _>(w.state.as_str())
    .bind::<Jsonb, _>(metadata)
    .bind::<Timestamptz, _>(w.created_at.as_datetime())
    .bind::<Nullable<Timestamptz>, _>(w.acknowledged_at.map(|t| t.as_datetime()))
    .bind::<Nullable<Timestamptz>, _>(w.started_at.map(|t| t.as_datetime()))
    .bind::<Nullable<Timestamptz>, _>(w.latest_heartbeat_at.map(|t| t.as_datetime()))
    .bind::<Nullable<Text>, _>(i.last_heartbeat.as_deref())
    .bind::<Nullable<Timestamptz>, _>(i.lease_until.map(|t| t.as_datetime()))
    .bind::<Nullable<Timestamptz>, _>(i.handed_out_at.map(|t| t.as_datetime()))
    .bind::<Nullable<Timestamptz>, _>(w.stop_requested_at.map(|t| t.as_datetime()))
    .bind::<Nullable<Timestamptz>, _>(w.stopped_at.map(|t| t.as_datetime()))
}
