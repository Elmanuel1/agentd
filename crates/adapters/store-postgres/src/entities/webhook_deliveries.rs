//! `webhook_deliveries` (V9): one row per event and endpoint, written in
//! the commit of the change it reports, so a delivery lands exactly when
//! its change does; the change feed hands each new row to the sender, which
//! records its end here (H-4).

use std::sync::Arc;

use ::sessions::types::SessionId;
use async_trait::async_trait;
use diesel::sql_types::{Array, BigInt, Text};
use diesel::OptionalExtension;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::{BackendError, WorkspaceId};
use webhooks::{Delivery, DeliveryMarks};

use crate::PgBackend;

/// Write `rows` in the transaction on `c`.
pub(crate) async fn insert_deliveries(
    c: &mut AsyncPgConnection,
    rows: &[Delivery],
) -> Result<(), diesel::result::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let col = |f: fn(&Delivery) -> &str| rows.iter().map(|r| f(r).to_owned()).collect::<Vec<_>>();
    diesel::sql_query(
        "INSERT INTO webhook_deliveries (webhook_id, endpoint_key, endpoint_url, body, next_attempt_at) \
         SELECT w, k, u, b, now() FROM unnest($1::text[], $2::text[], $3::text[], $4::text[]) AS d(w, k, u, b)",
    )
    .bind::<Array<Text>, _>(col(|r| &r.webhook_id))
    .bind::<Array<Text>, _>(col(|r| &r.endpoint_key))
    .bind::<Array<Text>, _>(col(|r| &r.endpoint_url))
    .bind::<Array<Text>, _>(col(|r| &r.body))
    .execute(c)
    .await
    .map(drop)
}

#[derive(diesel::QueryableByName)]
struct Org {
    #[diesel(sql_type = Text)]
    org_id: String,
}

/// The deliveries `build` makes for a session's status change, in its
/// commit: the organization read from the session's work item, which every
/// session has.
pub(crate) async fn status_changed(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
    build: impl FnOnce(&str) -> Vec<Delivery>,
) -> Result<(), diesel::result::Error> {
    let org: Option<Org> =
        diesel::sql_query("SELECT org_id FROM work_items WHERE workspace_id = $1 AND session_id = $2")
            .bind::<Text, _>(ws.as_str())
            .bind::<Text, _>(id.as_str())
            .get_result(c)
            .await
            .optional()?;
    let Some(org) = org else { return Ok(()) };
    insert_deliveries(c, &build(&org.org_id)).await
}

/// Write `rows` in a transaction of their own, for a change with no other
/// write to join (a failed refresh).
pub async fn publish(backend: &PgBackend, rows: &[Delivery]) -> Result<(), BackendError> {
    let mut conn = backend.diesel.write().await.map_err(BackendError::new)?;
    insert_deliveries(&mut conn, rows).await.map_err(BackendError::new)
}

/// Records each delivery's end.
#[derive(Debug, Clone)]
pub struct PgDeliveryMarks {
    backend: PgBackend,
}

impl PgDeliveryMarks {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Arc<Self> {
        Arc::new(Self { backend })
    }

    async fn mark(&self, column: &str, id: &str, key: &str, attempt: i64) -> Result<(), BackendError> {
        let mut conn = self.backend.diesel.write().await.map_err(BackendError::new)?;
        diesel::sql_query(format!(
            "UPDATE webhook_deliveries SET {column} = now(), attempt = $3 WHERE webhook_id = $1 AND endpoint_key = $2"
        ))
        .bind::<Text, _>(id)
        .bind::<Text, _>(key)
        .bind::<BigInt, _>(attempt)
        .execute(&mut conn)
        .await
        .map(drop)
        .map_err(BackendError::new)
    }
}

#[async_trait]
impl DeliveryMarks for PgDeliveryMarks {
    async fn delivered(&self, id: &str, key: &str, attempt: i64) -> Result<(), BackendError> {
        self.mark("delivered_at", id, key, attempt).await
    }

    async fn dropped(&self, id: &str, key: &str, attempt: i64) -> Result<(), BackendError> {
        self.mark("dropped_at", id, key, attempt).await
    }
}
