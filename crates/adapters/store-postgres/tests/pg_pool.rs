//! The pool's bounds on live Postgres: a statement past the statement timeout
//! fails, and a transaction left idle is ended (SH-10). Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use diesel_async::RunQueryDsl;
use store_postgres::{PgBackend, PoolSettings};

#[tokio::test]
async fn a_statement_over_the_timeout_fails_and_every_connection_carries_both_bounds() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let url = std::env::var("DATABASE_URL").unwrap();
    let settings = PoolSettings { statement_timeout: Duration::from_millis(500), ..PoolSettings::default() };
    let pg = PgBackend::connect_with(&url, settings).await.unwrap();
    let mut conn = pg.diesel.get().await.unwrap();
    let slow = diesel::sql_query("SELECT pg_sleep(2)").execute(&mut conn).await;
    let err = slow.unwrap_err().to_string();
    assert!(err.contains("statement timeout"), "{err}");

    #[derive(diesel::QueryableByName)]
    struct Setting {
        #[diesel(sql_type = diesel::sql_types::Text)]
        s: String,
    }
    let mut conn = pg.diesel.get().await.unwrap();
    let idle: Setting =
        diesel::sql_query("SELECT current_setting('idle_in_transaction_session_timeout') AS s")
            .get_result(&mut conn)
            .await
            .unwrap();
    assert_eq!(idle.s, "1min");
}
