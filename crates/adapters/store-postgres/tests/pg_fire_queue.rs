//! The fire queue over live Postgres: an occurrence is written once, claimed
//! by one worker at a time under its lease, freed for its backoff, and
//! settled only by the claim that holds its token. Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use chrono::{DateTime, Duration, DurationRound, Utc};
use diesel::QueryableByName;
use diesel::sql_types::{Integer, Nullable, Text, Timestamptz};
use diesel_async::RunQueryDsl;
use shared::ports::fire::{Claimed, Fire, FireClaims, Outcome};
use shared::ports::{FireTarget, ScheduleSpec, TenantRef};
use store_postgres::{PgBackend, PgFireClaims};

/// `claim_due` claims due rows of every test and the due count counts them, so the tests take turns.
static CLAIMS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

fn uniq(tag: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!("{tag}{}{}", Utc::now().timestamp_micros(), NEXT.fetch_add(1, Ordering::Relaxed))
}

fn spec(tag: &str) -> ScheduleSpec {
    ScheduleSpec {
        cron: "0 * * * *".into(),
        timezone: "UTC".into(),
        target: FireTarget::new("queuetest", uniq(tag)),
        tenant: TenantRef {
            org_id: "org_q".into(),
            workspace_id: "wrkspc_q".into(),
            api_key_id: "apikey_q".into(),
        },
    }
}

/// This hour's occurrence of `spec`, due a minute ago.
fn due_fire(spec: &ScheduleSpec) -> Fire {
    let scheduled_at = Utc::now().duration_trunc(Duration::hours(1)).unwrap();
    Fire { spec: spec.clone(), scheduled_at, due_at: Utc::now() - Duration::minutes(1) }
}

/// The claims of `fire` among everything due.
async fn claim(q: &PgFireClaims, fire: &Fire) -> Vec<Claimed> {
    let all = q.claim_due(10_000).await.unwrap();
    all.into_iter().filter(|c| c.key.target == fire.spec.target.key()).collect()
}

#[derive(QueryableByName, Debug, PartialEq)]
struct Row {
    #[diesel(sql_type = Nullable<Timestamptz>)]
    due_at: Option<DateTime<Utc>>,
    #[diesel(sql_type = Integer)]
    attempts: i32,
    #[diesel(sql_type = diesel::sql_types::Bool)]
    done: bool,
    #[diesel(sql_type = Nullable<Text>)]
    outcome: Option<String>,
}

async fn row(pg: &PgBackend, fire: &Fire) -> Row {
    let mut conn = pg.diesel.get().await.unwrap();
    diesel::sql_query("SELECT due_at, attempts, done, outcome FROM fire_claims WHERE target = $1")
        .bind::<Text, _>(fire.spec.target.key())
        .get_result(&mut conn)
        .await
        .unwrap()
}

async fn sql(pg: &PgBackend, statement: &str, fire: &Fire) {
    let mut conn = pg.diesel.write().await.unwrap();
    diesel::sql_query(statement).bind::<Text, _>(fire.spec.target.key()).execute(&mut conn).await.unwrap();
}

#[tokio::test]
async fn an_enqueued_occurrence_is_claimed_once_across_two_workers() {
    if !enabled() {
        return;
    }
    let _turn = CLAIMS.lock().await;
    let pg = backend().await;
    let (a, b) = (PgFireClaims::new(pg.clone()), PgFireClaims::new(pg.clone()));
    let fire = due_fire(&spec("once"));
    a.enqueue(std::slice::from_ref(&fire)).await.unwrap();
    let (ca, cb) = tokio::join!(claim(&a, &fire), claim(&b, &fire));
    assert_eq!(ca.len() + cb.len(), 1, "{ca:?} {cb:?}");
    let claimed = ca.into_iter().chain(cb).next().unwrap();
    assert_eq!((claimed.token, claimed.spec.as_ref()), (1, Some(&fire.spec)));
    assert!(claim(&a, &fire).await.is_empty(), "its lease holds");
}

#[tokio::test]
async fn a_claim_whose_worker_stopped_is_taken_again_after_its_lease() {
    if !enabled() {
        return;
    }
    let _turn = CLAIMS.lock().await;
    let pg = backend().await;
    let q = PgFireClaims::new(pg.clone());
    let fire = due_fire(&spec("lease"));
    q.enqueue(std::slice::from_ref(&fire)).await.unwrap();
    assert_eq!(claim(&q, &fire).await.len(), 1);
    sql(&pg, "UPDATE fire_claims SET claimed_at = now() - interval '119 seconds' WHERE target = $1", &fire)
        .await;
    assert!(claim(&q, &fire).await.is_empty(), "within its 2-minute lease");
    sql(&pg, "UPDATE fire_claims SET claimed_at = now() - interval '121 seconds' WHERE target = $1", &fire)
        .await;
    let again = claim(&q, &fire).await;
    assert_eq!(again.iter().map(|c| c.token).collect::<Vec<_>>(), [2]);
}

#[tokio::test]
async fn a_retried_fire_waits_for_its_backoff() {
    if !enabled() {
        return;
    }
    let _turn = CLAIMS.lock().await;
    let pg = backend().await;
    let q = PgFireClaims::new(pg.clone());
    let fire = due_fire(&spec("retry"));
    q.enqueue(std::slice::from_ref(&fire)).await.unwrap();
    let claimed = claim(&q, &fire).await.pop().unwrap();
    let later = Utc::now() + Duration::hours(1);
    assert!(q.retry(&claimed, later).await.unwrap());
    assert!(claim(&q, &fire).await.is_empty(), "not before its backoff");
    assert_eq!(row(&pg, &fire).await.due_at.map(|d| d.timestamp_micros()), Some(later.timestamp_micros()));
    sql(&pg, "UPDATE fire_claims SET due_at = now() - interval '1 second' WHERE target = $1", &fire).await;
    assert_eq!(claim(&q, &fire).await.iter().map(|c| c.token).collect::<Vec<_>>(), [2], "at once after it");
}

#[tokio::test]
async fn enqueuing_an_occurrence_twice_keeps_the_first_due_time() {
    if !enabled() {
        return;
    }
    let _turn = CLAIMS.lock().await;
    let pg = backend().await;
    let q = PgFireClaims::new(pg.clone());
    let first = due_fire(&spec("twice"));
    q.enqueue(std::slice::from_ref(&first)).await.unwrap();
    let second = Fire { due_at: first.due_at + Duration::minutes(5), ..first.clone() };
    q.enqueue(&[second]).await.unwrap();
    let stored = row(&pg, &first).await;
    assert_eq!(stored.due_at.map(|d| d.timestamp_micros()), Some(first.due_at.timestamp_micros()));
    assert_eq!((stored.attempts, stored.done, stored.outcome), (0, false, None));
}

#[tokio::test]
async fn a_done_fire_records_ran_skipped_or_dropped() {
    if !enabled() {
        return;
    }
    let _turn = CLAIMS.lock().await;
    let pg = backend().await;
    let q = PgFireClaims::new(pg.clone());
    for outcome in [Outcome::Ran, Outcome::Skipped, Outcome::Dropped] {
        let fire = due_fire(&spec(outcome.as_str()));
        q.enqueue(std::slice::from_ref(&fire)).await.unwrap();
        let claimed = claim(&q, &fire).await.pop().unwrap();
        assert!(q.finish(&claimed, outcome).await.unwrap());
        let stored = row(&pg, &fire).await;
        assert_eq!((stored.done, stored.outcome.as_deref()), (true, Some(outcome.as_str())));
        sql(&pg, "UPDATE fire_claims SET claimed_at = now() - interval '1 hour' WHERE target = $1", &fire)
            .await;
        assert!(claim(&q, &fire).await.is_empty(), "a done fire never runs again");
    }
}

#[tokio::test]
async fn a_worker_whose_lease_was_taken_over_cannot_finish_or_retry_the_fire() {
    if !enabled() {
        return;
    }
    let _turn = CLAIMS.lock().await;
    let pg = backend().await;
    let q = PgFireClaims::new(pg.clone());
    let fire = due_fire(&spec("lost"));
    q.enqueue(std::slice::from_ref(&fire)).await.unwrap();
    let stale = claim(&q, &fire).await.pop().unwrap();
    sql(&pg, "UPDATE fire_claims SET claimed_at = now() - interval '121 seconds' WHERE target = $1", &fire)
        .await;
    let current = claim(&q, &fire).await.pop().unwrap();
    let before = row(&pg, &fire).await;

    assert!(!q.finish(&stale, Outcome::Ran).await.unwrap());
    assert!(!q.retry(&stale, Utc::now()).await.unwrap());
    assert_eq!(row(&pg, &fire).await, before, "the row stays as the current claim left it");
    assert!(q.finish(&current, Outcome::Ran).await.unwrap());
}

async fn exists(pg: &PgBackend, fire: &Fire) -> bool {
    #[derive(QueryableByName)]
    struct Found {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    let mut conn = pg.diesel.get().await.unwrap();
    let found: Found = diesel::sql_query("SELECT count(*) AS n FROM fire_claims WHERE target = $1")
        .bind::<Text, _>(fire.spec.target.key())
        .get_result(&mut conn)
        .await
        .unwrap();
    found.n > 0
}

#[tokio::test]
async fn done_rows_older_than_30_days_are_swept_newer_and_undone_rows_stay() {
    if !enabled() {
        return;
    }
    let _turn = CLAIMS.lock().await;
    let pg = backend().await;
    let q = PgFireClaims::new(pg.clone());
    let at = |days: i64| Utc::now().duration_trunc(Duration::hours(1)).unwrap() - Duration::days(days);
    let fire = |tag: &str, days: i64| {
        let s = spec(tag);
        Fire { scheduled_at: at(days), due_at: at(days), spec: s }
    };
    let (old_done, new_done, old_undone) = (fire("olddone", 31), fire("newdone", 29), fire("oldundone", 31));
    q.enqueue(&[old_done.clone(), new_done.clone(), old_undone.clone()]).await.unwrap();
    for f in [&old_done, &new_done] {
        sql(&pg, "UPDATE fire_claims SET done = true, outcome = 'ran' WHERE target = $1", f).await;
    }

    assert!(q.sweep().await.unwrap() >= 1);

    assert!(!exists(&pg, &old_done).await, "done and 31 days old");
    assert!(exists(&pg, &new_done).await, "done but 29 days old");
    assert!(exists(&pg, &old_undone).await, "a queued fire stays until it is done");
    sql(&pg, "DELETE FROM fire_claims WHERE target = $1", &old_undone).await;
}

#[tokio::test]
async fn the_due_count_holds_the_due_fires_not_done() {
    if !enabled() {
        return;
    }
    let _turn = CLAIMS.lock().await;
    let pg = backend().await;
    let q = PgFireClaims::new(pg.clone());
    let before = q.count_due().await.unwrap();
    let fire = due_fire(&spec("count"));
    let later = Fire { due_at: Utc::now() + Duration::hours(1), ..due_fire(&spec("countlater")) };
    q.enqueue(&[fire.clone(), later]).await.unwrap();
    assert_eq!(q.count_due().await.unwrap(), before + 1);
    let claimed = claim(&q, &fire).await.pop().unwrap();
    assert!(q.finish(&claimed, Outcome::Ran).await.unwrap());
    assert_eq!(q.count_due().await.unwrap(), before);
}
