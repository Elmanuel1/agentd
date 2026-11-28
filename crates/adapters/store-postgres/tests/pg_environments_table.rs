//! The store's shape after V28 and V29: environments are one plain table, and there is no history
//! schema. Gated by `AGENTD_PG_TESTS=1`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use diesel::sql_types::{BigInt, Text};
use diesel_async::RunQueryDsl;
use store_postgres::PgBackend;

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

#[derive(diesel::QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[derive(diesel::QueryableByName)]
struct Line {
    #[diesel(sql_type = Text)]
    line: String,
}

async fn count(pg: &PgBackend, sql: &str) -> i64 {
    let mut conn = pg.diesel.get().await.unwrap();
    let row: Count = diesel::sql_query(sql).get_result(&mut *conn).await.unwrap();
    row.n
}

#[tokio::test]
async fn environments_is_a_plain_table_with_no_revision_machinery() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let kind =
        count(&pg, "SELECT count(*) AS n FROM pg_class WHERE relname = 'environments' AND relkind = 'r'")
            .await;
    let revisions =
        count(&pg, "SELECT count(*) AS n FROM pg_class WHERE relname = 'environment_revisions'").await;
    let functions = count(
        &pg,
        "SELECT count(*) AS n FROM pg_proc WHERE proname IN
           ('environments_revise', 'environments_soft_delete', 'revisions_append_only')",
    )
    .await;
    let key = count(
        &pg,
        "SELECT count(*) AS n FROM pg_indexes WHERE tablename = 'environments'
           AND indexdef LIKE '%UNIQUE INDEX environments_pkey ON public.environments USING btree (org_id, environment_id)'",
    )
    .await;
    assert_eq!((kind, revisions, functions, key), (1, 0, 0, 1));
}

#[tokio::test]
async fn the_store_keeps_no_history_schema() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    assert_eq!(count(&pg, "SELECT count(*) AS n FROM pg_namespace WHERE nspname = 'history'").await, 0);
}

#[tokio::test]
async fn the_list_index_orders_by_creation_then_id_descending() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let mut conn = pg.diesel.get().await.unwrap();
    let row: Line = diesel::sql_query(
        "SELECT indexdef AS line FROM pg_indexes WHERE indexname = 'environments_org_created_idx'",
    )
    .get_result(&mut *conn)
    .await
    .unwrap();
    assert!(row.line.ends_with("(org_id, created_at DESC, environment_id DESC)"), "{}", row.line);
}
