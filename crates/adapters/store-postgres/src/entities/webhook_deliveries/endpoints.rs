//! `webhook_endpoint_states`: one statement per failed attempt, per disable,
//! per run of failures ended, and per 10 s read on each replica; the table
//! holds one row per configured endpoint, so the read takes it whole.

use chrono::{DateTime, Utc};
use diesel::QueryableByName;
use diesel::sql_types::{Array, Bool, Nullable, Text, Timestamptz};
use diesel_async::RunQueryDsl;
use shared::{BackendError, Timestamp};
use webhooks::states::{Configured, DisabledReason, EndpointState, NativeEndpointStates};

use super::PgWebhooks;

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type = Text)]
    endpoint_key: String,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    failing_since: Option<DateTime<Utc>>,
    #[diesel(sql_type = Nullable<Text>)]
    disabled_reason: Option<String>,
}

#[derive(QueryableByName)]
struct Disabled {
    #[diesel(sql_type = Bool)]
    newly: bool,
}

impl NativeEndpointStates for PgWebhooks {
    async fn configure(&self, endpoints: &[Configured]) -> Result<(), BackendError> {
        let col = |f: fn(&Configured) -> &str| endpoints.iter().map(f).collect::<Vec<_>>();
        let mut conn = self.backend.diesel.write().await.map_err(BackendError::new)?;
        // A changed URL or secret is a new endpoint: its disable and its run of failures end.
        diesel::sql_query(
            "INSERT INTO webhook_endpoint_states (endpoint_key, org_id, config_digest)
             SELECT * FROM unnest($1::text[], $2::text[], $3::text[])
             ON CONFLICT (endpoint_key) DO UPDATE SET org_id = excluded.org_id,
               config_digest = excluded.config_digest,
               disabled_at = CASE WHEN webhook_endpoint_states.config_digest = excluded.config_digest
                 THEN webhook_endpoint_states.disabled_at END,
               disabled_reason = CASE WHEN webhook_endpoint_states.config_digest = excluded.config_digest
                 THEN webhook_endpoint_states.disabled_reason END,
               failing_since = CASE WHEN webhook_endpoint_states.config_digest = excluded.config_digest
                 THEN webhook_endpoint_states.failing_since END",
        )
        .bind::<Array<Text>, _>(col(|e| &e.endpoint_key))
        .bind::<Array<Text>, _>(col(|e| &e.org_id))
        .bind::<Array<Text>, _>(col(|e| &e.digest))
        .execute(&mut conn)
        .await
        .map(drop)
        .map_err(BackendError::new)
    }

    async fn read(&self) -> Result<Vec<EndpointState>, BackendError> {
        let mut conn = self.backend.diesel.get().await.map_err(BackendError::new)?;
        let rows: Vec<Row> = diesel::sql_query(
            "SELECT endpoint_key, failing_since, CASE WHEN disabled_at IS NOT NULL THEN disabled_reason END
               AS disabled_reason
             FROM webhook_endpoint_states WHERE failing_since IS NOT NULL OR disabled_at IS NOT NULL",
        )
        .load(&mut conn)
        .await
        .map_err(BackendError::new)?;
        Ok(rows
            .into_iter()
            .map(|r| EndpointState {
                endpoint_key: r.endpoint_key,
                failing_since: r.failing_since.map(Timestamp::from_datetime),
                disabled: r
                    .disabled_reason
                    .map(|d| DisabledReason::parse(&d).unwrap_or(DisabledReason::Failing)),
            })
            .collect())
    }

    async fn failed(&self, endpoint: &Configured) -> Result<bool, BackendError> {
        let mut conn = self.backend.diesel.write().await.map_err(BackendError::new)?;
        // One statement, so `now()` is this failure's time: `disabled_at = now()` only when it disabled it.
        let disabled: Disabled = diesel::sql_query(
            "INSERT INTO webhook_endpoint_states (endpoint_key, org_id, failing_since, config_digest)
             VALUES ($1, $2, now(), $3)
             ON CONFLICT (endpoint_key) DO UPDATE SET
               failing_since = coalesce(webhook_endpoint_states.failing_since, now()),
               disabled_at = CASE WHEN webhook_endpoint_states.failing_since < now() - interval '120 hours'
                 THEN coalesce(webhook_endpoint_states.disabled_at, now())
                 ELSE webhook_endpoint_states.disabled_at END,
               disabled_reason = CASE WHEN webhook_endpoint_states.failing_since < now() - interval '120 hours'
                 THEN coalesce(webhook_endpoint_states.disabled_reason, 'failing')
                 ELSE webhook_endpoint_states.disabled_reason END
             RETURNING coalesce(disabled_at = now(), false) AS newly",
        )
        .bind::<Text, _>(&endpoint.endpoint_key)
        .bind::<Text, _>(&endpoint.org_id)
        .bind::<Text, _>(&endpoint.digest)
        .get_result(&mut conn)
        .await
        .map_err(BackendError::new)?;
        Ok(disabled.newly)
    }

    async fn disable(&self, endpoint: &Configured, why: DisabledReason) -> Result<bool, BackendError> {
        let mut conn = self.backend.diesel.write().await.map_err(BackendError::new)?;
        let disabled: Disabled = diesel::sql_query(
            "INSERT INTO webhook_endpoint_states (endpoint_key, org_id, config_digest, disabled_at, disabled_reason)
             VALUES ($1, $2, $3, now(), $4)
             ON CONFLICT (endpoint_key) DO UPDATE SET
               disabled_at = coalesce(webhook_endpoint_states.disabled_at, now()),
               disabled_reason = coalesce(webhook_endpoint_states.disabled_reason, excluded.disabled_reason)
             RETURNING disabled_at = now() AS newly",
        )
        .bind::<Text, _>(&endpoint.endpoint_key)
        .bind::<Text, _>(&endpoint.org_id)
        .bind::<Text, _>(&endpoint.digest)
        .bind::<Text, _>(why.as_str())
        .get_result(&mut conn)
        .await
        .map_err(BackendError::new)?;
        Ok(disabled.newly)
    }

    async fn clear_failing(&self, endpoint_key: &str, since: Timestamp) -> Result<(), BackendError> {
        let mut conn = self.backend.diesel.write().await.map_err(BackendError::new)?;
        diesel::sql_query(
            "UPDATE webhook_endpoint_states SET failing_since = NULL WHERE endpoint_key = $1 AND failing_since = $2",
        )
        .bind::<Text, _>(endpoint_key)
        .bind::<Timestamptz, _>(since.as_datetime())
        .execute(&mut conn)
        .await
        .map(drop)
        .map_err(BackendError::new)
    }
}

impl PgWebhooks {
    /// Enable `org`'s endpoint `endpoint_key` again and end its run of failures; `false` when it has no state row.
    pub async fn enable(&self, org: &str, endpoint_key: &str) -> Result<bool, BackendError> {
        let mut conn = self.backend.diesel.write().await.map_err(BackendError::new)?;
        let enabled = diesel::sql_query(
            "UPDATE webhook_endpoint_states SET disabled_at = NULL, disabled_reason = NULL, failing_since = NULL
             WHERE endpoint_key = $1 AND org_id = $2",
        )
        .bind::<Text, _>(endpoint_key)
        .bind::<Text, _>(org)
        .execute(&mut conn)
        .await
        .map_err(BackendError::new)?;
        Ok(enabled == 1)
    }
}
