//! The claim and every write fenced on it. Two instances never hold one
//! row: the select's `FOR NO KEY UPDATE SKIP LOCKED` and the update run in
//! one transaction, the update repeats the due conditions so a row claimed in
//! between is not returned, and every time is Postgres `now()` (D19).

use chrono::{DateTime, Utc};
use diesel::QueryableByName;
use diesel::sql_types::{Array, BigInt, Double, Nullable, SmallInt, Text, Timestamptz};
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::{BackendError, Timestamp};
use webhooks::queue::{Attempted, ClaimRef, Claimed, NativeDeliveryQueue, Next};

use super::PgWebhooks;

#[derive(QueryableByName)]
struct Due {
    #[diesel(sql_type = Text)]
    webhook_id: String,
    #[diesel(sql_type = Text)]
    endpoint_key: String,
}

#[derive(QueryableByName)]
struct Taken {
    #[diesel(sql_type = Text)]
    webhook_id: String,
    #[diesel(sql_type = Text)]
    endpoint_key: String,
    #[diesel(sql_type = BigInt)]
    claim_seq: i64,
    #[diesel(sql_type = BigInt)]
    attempt: i64,
    #[diesel(sql_type = Text)]
    endpoint_url: String,
    #[diesel(sql_type = Text)]
    body: String,
}

#[derive(QueryableByName)]
struct Oldest {
    #[diesel(sql_type = Nullable<Timestamptz>)]
    due: Option<DateTime<Utc>>,
}

impl From<Taken> for Claimed {
    fn from(t: Taken) -> Self {
        let claim =
            ClaimRef { webhook_id: t.webhook_id, endpoint_key: t.endpoint_key, claim_seq: t.claim_seq };
        Self { claim, attempt: t.attempt, endpoint_url: t.endpoint_url, body: t.body }
    }
}

/// The fence every write after a claim carries: `$1` to `$3`.
const CLAIMED: &str = "webhook_id = $1 AND endpoint_key = $2 AND claim_seq = $3";

impl PgWebhooks {
    /// Run `sql`, fenced on `claim` and taking its own binds from `$4` on through `bind`.
    async fn fenced<'q>(
        &self,
        sql: String,
        claim: &'q ClaimRef,
        bind: impl FnOnce(
            diesel::query_builder::BoxedSqlQuery<'q, diesel::pg::Pg, diesel::query_builder::SqlQuery>,
        ) -> diesel::query_builder::BoxedSqlQuery<
            'q,
            diesel::pg::Pg,
            diesel::query_builder::SqlQuery,
        >,
    ) -> Result<bool, BackendError> {
        let query = diesel::sql_query(sql)
            .into_boxed()
            .bind::<Text, _>(claim.webhook_id.as_str())
            .bind::<Text, _>(claim.endpoint_key.as_str())
            .bind::<BigInt, _>(claim.claim_seq);
        let mut conn = self.backend.diesel.write().await.map_err(BackendError::new)?;
        let written = bind(query).execute(&mut conn).await.map_err(BackendError::new)?;
        Ok(written == 1)
    }
}

impl NativeDeliveryQueue for PgWebhooks {
    async fn claim(&self, limit: usize, skip: &[String]) -> Result<Vec<Claimed>, BackendError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut conn = self.backend.diesel.write().await.map_err(BackendError::new)?;
        let taken = conn
            .transaction::<_, diesel::result::Error, _>(async move |c| {
                let due: Vec<Due> = diesel::sql_query(
                    "SELECT webhook_id, endpoint_key FROM webhook_deliveries
                     WHERE delivered_at IS NULL AND dropped_at IS NULL AND next_attempt_at <= now()
                       AND (claimed_until IS NULL OR claimed_until < now()) AND endpoint_key <> ALL($1)
                     ORDER BY next_attempt_at LIMIT $2 FOR NO KEY UPDATE SKIP LOCKED",
                )
                .bind::<Array<Text>, _>(skip)
                .bind::<BigInt, _>(limit)
                .load(c)
                .await?;
                if due.is_empty() {
                    return Ok(Vec::new());
                }
                let ids: Vec<&str> = due.iter().map(|d| d.webhook_id.as_str()).collect();
                let keys: Vec<&str> = due.iter().map(|d| d.endpoint_key.as_str()).collect();
                // A claim whose `claimed_until` is still set lapsed and may have sent: it counts as an attempt.
                diesel::sql_query(
                    "UPDATE webhook_deliveries AS d SET claimed_until = now() + interval '60 seconds',
                       claim_seq = d.claim_seq + 1,
                       attempt = least(3, d.attempt + (d.claimed_until IS NOT NULL)::int)
                     FROM unnest($1::text[], $2::text[]) AS v(webhook_id, endpoint_key)
                     WHERE d.webhook_id = v.webhook_id AND d.endpoint_key = v.endpoint_key
                       AND d.delivered_at IS NULL AND d.dropped_at IS NULL
                       AND (d.claimed_until IS NULL OR d.claimed_until < now())
                     RETURNING d.webhook_id, d.endpoint_key, d.claim_seq, d.attempt, d.endpoint_url, d.body",
                )
                .bind::<Array<Text>, _>(&ids)
                .bind::<Array<Text>, _>(&keys)
                .load::<Taken>(c)
                .await
            })
            .await
            .map_err(BackendError::new)?;
        Ok(taken.into_iter().map(Claimed::from).collect())
    }

    async fn release(&self, claim: &ClaimRef) -> Result<bool, BackendError> {
        let sql = format!("UPDATE webhook_deliveries SET claimed_until = NULL WHERE {CLAIMED}");
        self.fenced(sql, claim, |q| q).await
    }

    async fn drop_unsent(&self, claim: &ClaimRef, why: &str) -> Result<bool, BackendError> {
        let sql = format!(
            "UPDATE webhook_deliveries SET dropped_at = now(), claimed_until = NULL, last_error = $4 WHERE {CLAIMED}"
        );
        let why = why.to_owned();
        self.fenced(sql, claim, move |q| q.bind::<Text, _>(why)).await
    }

    async fn record(&self, claim: &ClaimRef, attempted: &Attempted) -> Result<bool, BackendError> {
        let end = match attempted.next {
            Next::Delivered => "delivered_at = now()",
            Next::Dropped => "dropped_at = now()",
            Next::Retry(_) => "next_attempt_at = now() + make_interval(secs => $6)",
        };
        let sql = format!(
            "UPDATE webhook_deliveries SET attempt = attempt + 1, {end}, claimed_until = NULL,
               last_status = $4, last_error = $5
             WHERE {CLAIMED}"
        );
        let status = attempted.status.and_then(|s| i16::try_from(s).ok());
        let error = attempted.error.clone();
        let next = attempted.next;
        self.fenced(sql, claim, move |q| {
            let q = q.bind::<Nullable<SmallInt>, _>(status).bind::<Nullable<Text>, _>(error);
            match next {
                Next::Retry(wait) => q.bind::<Double, _>(wait.as_secs_f64()),
                Next::Delivered | Next::Dropped => q,
            }
        })
        .await
    }

    async fn oldest_due(&self) -> Result<Option<Timestamp>, BackendError> {
        let mut conn = self.backend.diesel.get().await.map_err(BackendError::new)?;
        let oldest: Oldest = diesel::sql_query(
            "SELECT min(next_attempt_at) AS due FROM webhook_deliveries
             WHERE delivered_at IS NULL AND dropped_at IS NULL AND next_attempt_at <= now()",
        )
        .get_result(&mut conn)
        .await
        .map_err(BackendError::new)?;
        Ok(oldest.due.map(Timestamp::from_datetime))
    }
}
