//! The queue's rows: an occurrence is written once with its spec and due
//! time, claimed by one worker at a time under a 2-minute lease, and settled
//! only by the claim that still holds its token (the row's `attempts`).

use chrono::{DateTime, Utc};
use diesel::QueryableByName;
use diesel::sql_types::{Array, BigInt, Integer, Jsonb, Text, Timestamptz};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::ports::fire::{Claimed, Fire, FireError, FireKey, NativeFireClaims, Outcome};

use super::{PgFireClaims, unavailable};

#[derive(QueryableByName)]
struct Due {
    #[diesel(sql_type = Text)]
    workspace_id: String,
    #[diesel(sql_type = Text)]
    target: String,
    #[diesel(sql_type = Timestamptz)]
    scheduled_at: DateTime<Utc>,
}

#[derive(QueryableByName)]
struct Taken {
    #[diesel(sql_type = Text)]
    workspace_id: String,
    #[diesel(sql_type = Text)]
    target: String,
    #[diesel(sql_type = Timestamptz)]
    scheduled_at: DateTime<Utc>,
    #[diesel(sql_type = diesel::sql_types::Nullable<Jsonb>)]
    spec: Option<serde_json::Value>,
    #[diesel(sql_type = Integer)]
    attempts: i32,
}

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

/// Write `fires` in the transaction on `c`; an occurrence already queued keeps its row.
pub(crate) async fn enqueue_in(
    c: &mut AsyncPgConnection,
    fires: &[Fire],
) -> Result<(), diesel::result::Error> {
    if fires.is_empty() {
        return Ok(());
    }
    let workspaces: Vec<&str> = fires.iter().map(|f| f.spec.tenant.workspace_id.as_str()).collect();
    let targets: Vec<String> = fires.iter().map(|f| f.spec.target.key()).collect();
    let times: Vec<DateTime<Utc>> = fires.iter().map(|f| f.scheduled_at).collect();
    let specs = fires
        .iter()
        .map(|f| serde_json::to_value(&f.spec))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| diesel::result::Error::SerializationError(e.into()))?;
    let dues: Vec<DateTime<Utc>> = fires.iter().map(|f| f.due_at).collect();
    diesel::sql_query(
        "INSERT INTO fire_claims (workspace_id, target, scheduled_at, spec, due_at, claimed_at, attempts)
         SELECT v.w, v.t, v.s, v.sp, v.d, '-infinity', 0
         FROM unnest($1::text[], $2::text[], $3::timestamptz[], $4::jsonb[], $5::timestamptz[]) AS v(w, t, s, sp, d)
         ON CONFLICT (workspace_id, target, scheduled_at) DO NOTHING",
    )
    .bind::<Array<Text>, _>(&workspaces)
    .bind::<Array<Text>, _>(&targets)
    .bind::<Array<Timestamptz>, _>(&times)
    .bind::<Array<Jsonb>, _>(&specs)
    .bind::<Array<Timestamptz>, _>(&dues)
    .execute(c)
    .await
    .map(drop)
}

/// Lease `due` to this claim; nothing to write when nothing is due.
async fn take_lease(c: &mut AsyncPgConnection, due: &[Due]) -> Result<Vec<Taken>, diesel::result::Error> {
    if due.is_empty() {
        return Ok(Vec::new());
    }
    let workspaces: Vec<&str> = due.iter().map(|d| d.workspace_id.as_str()).collect();
    let targets: Vec<&str> = due.iter().map(|d| d.target.as_str()).collect();
    let times: Vec<DateTime<Utc>> = due.iter().map(|d| d.scheduled_at).collect();
    diesel::sql_query(
        "UPDATE fire_claims SET claimed_at = now(), attempts = fire_claims.attempts + 1
         FROM unnest($1::text[], $2::text[], $3::timestamptz[]) AS v(w, t, s)
         WHERE fire_claims.workspace_id = v.w AND fire_claims.target = v.t AND fire_claims.scheduled_at = v.s
         RETURNING fire_claims.workspace_id, fire_claims.target, fire_claims.scheduled_at, fire_claims.spec,
           fire_claims.attempts",
    )
    .bind::<Array<Text>, _>(&workspaces)
    .bind::<Array<Text>, _>(&targets)
    .bind::<Array<Timestamptz>, _>(&times)
    .load(c)
    .await
}

fn claimed(t: Taken) -> Claimed {
    let spec = t.spec.and_then(|doc| serde_json::from_value(doc).ok());
    let key = FireKey { workspace_id: t.workspace_id, target: t.target, scheduled_at: t.scheduled_at };
    Claimed { key, token: t.attempts, spec }
}

impl NativeFireClaims for PgFireClaims {
    async fn enqueue(&self, fires: &[Fire]) -> Result<(), FireError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        enqueue_in(&mut conn, fires).await.map_err(unavailable)
    }

    async fn claim_due(&self, limit: usize) -> Result<Vec<Claimed>, FireError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let taken = conn
            .transaction::<_, diesel::result::Error, _>(async move |c| {
                let due: Vec<Due> = diesel::sql_query(
                    "SELECT workspace_id, target, scheduled_at FROM fire_claims
                     WHERE NOT done AND due_at <= now() AND claimed_at < now() - interval '120 seconds'
                     ORDER BY due_at LIMIT $1 FOR UPDATE SKIP LOCKED",
                )
                .bind::<BigInt, _>(limit)
                .load(c)
                .await?;
                take_lease(c, &due).await
            })
            .await
            .map_err(unavailable)?;
        Ok(taken.into_iter().map(claimed).collect())
    }

    async fn finish(&self, claim: &Claimed, outcome: Outcome) -> Result<bool, FireError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let written = diesel::sql_query(
            "UPDATE fire_claims SET done = true, outcome = $5
             WHERE workspace_id = $1 AND target = $2 AND scheduled_at = $3 AND attempts = $4",
        )
        .bind::<Text, _>(&claim.key.workspace_id)
        .bind::<Text, _>(&claim.key.target)
        .bind::<Timestamptz, _>(claim.key.scheduled_at)
        .bind::<Integer, _>(claim.token)
        .bind::<Text, _>(outcome.as_str())
        .execute(&mut conn)
        .await
        .map_err(unavailable)?;
        Ok(written == 1)
    }

    async fn retry(&self, claim: &Claimed, at: DateTime<Utc>) -> Result<bool, FireError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let written = diesel::sql_query(
            "UPDATE fire_claims SET due_at = $5, claimed_at = '-infinity'
             WHERE workspace_id = $1 AND target = $2 AND scheduled_at = $3 AND attempts = $4 AND NOT done",
        )
        .bind::<Text, _>(&claim.key.workspace_id)
        .bind::<Text, _>(&claim.key.target)
        .bind::<Timestamptz, _>(claim.key.scheduled_at)
        .bind::<Integer, _>(claim.token)
        .bind::<Timestamptz, _>(at)
        .execute(&mut conn)
        .await
        .map_err(unavailable)?;
        Ok(written == 1)
    }
}

impl PgFireClaims {
    /// The fires due and not done.
    pub async fn count_due(&self) -> Result<i64, FireError> {
        let mut conn = self.backend.diesel.get().await.map_err(unavailable)?;
        let count: Count =
            diesel::sql_query("SELECT count(*) AS n FROM fire_claims WHERE NOT done AND due_at <= now()")
                .get_result(&mut conn)
                .await
                .map_err(unavailable)?;
        Ok(count.n)
    }
}
