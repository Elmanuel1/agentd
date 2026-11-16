//! The tracked tables record their writer and announce their deletes (V16).
//! Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`; the delete messages are read
//! from a test slot, so those tests also need `AGENTD_CDC_TESTS=1` (logical WAL).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use diesel::sql_types::{Nullable, Text};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::{with_writer, RequestId, Timestamp, Writer};
use store_postgres::PgBackend;

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

fn cdc_enabled() -> bool {
    enabled() && std::env::var("AGENTD_CDC_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

fn vault_id() -> String {
    format!("vlt_capture{}", Timestamp::now().as_datetime().timestamp_nanos_opt().unwrap())
}

/// Run `sql` (with `$1` the vault id) on a write checkout as `writer`.
async fn write_as(pg: &PgBackend, writer: &Writer, sql: &str, vault: &str) {
    with_writer(writer.clone(), async {
        let mut conn = pg.diesel.write().await.unwrap();
        diesel::sql_query(sql).bind::<Text, _>(vault).execute(&mut *conn).await.unwrap();
    })
    .await;
}

const INSERT: &str = "INSERT INTO vaults (workspace_id, vault_id, display_name, created_at, doc)
                      VALUES ('wrkspc_capture', $1, 'v', now(), '{}')";

#[derive(diesel::QueryableByName)]
struct Stamp {
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Nullable<Text>)]
    request_id: Option<String>,
}

#[tokio::test]
async fn an_update_records_its_own_writer_not_the_inserter() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let vault = vault_id();
    let (a, b) = (Writer::fresh("apikey_a"), Writer::new("apikey_b", &RequestId::generate()));
    write_as(&pg, &a, INSERT, &vault).await;
    write_as(&pg, &b, "UPDATE vaults SET display_name = 'w' WHERE vault_id = $1", &vault).await;
    let mut conn = pg.diesel.get().await.unwrap();
    let row: Stamp = diesel::sql_query("SELECT actor, request_id FROM vaults WHERE vault_id = $1")
        .bind::<Text, _>(&vault)
        .get_result(&mut *conn)
        .await
        .unwrap();
    assert_eq!((row.actor, row.request_id), (b.actor, Some(b.request_id)));
}

#[derive(diesel::QueryableByName)]
struct Decoded {
    #[diesel(sql_type = Text)]
    data: String,
}

/// The `agentd-history` messages a test slot decoded that name `vault`.
async fn messages(conn: &mut diesel_async::AsyncPgConnection, slot: &str, vault: &str) -> Vec<String> {
    let rows: Vec<Decoded> = diesel::sql_query(
        "SELECT data FROM pg_logical_slot_peek_changes($1, NULL, NULL) WHERE data LIKE 'message:%'",
    )
    .bind::<Text, _>(slot)
    .load(conn)
    .await
    .unwrap();
    rows.into_iter().map(|r| r.data).filter(|d| d.contains("agentd-history") && d.contains(vault)).collect()
}

#[tokio::test]
async fn a_delete_emits_one_message_with_the_row_only_when_it_commits() {
    if !cdc_enabled() {
        return;
    }
    let pg = backend().await;
    let mut conn = pg.diesel.get().await.unwrap();
    let slot = format!("history_capture_{}", Timestamp::now().as_datetime().timestamp_micros());
    diesel::sql_query("SELECT 1 FROM pg_create_logical_replication_slot($1, 'test_decoding')")
        .bind::<Text, _>(&slot)
        .execute(&mut *conn)
        .await
        .unwrap();
    let (vault, deleter) = (vault_id(), Writer::fresh("adminkey_d"));
    write_as(&pg, &Writer::fresh("apikey_a"), INSERT, &vault).await;
    let rolled_back = conn
        .transaction::<(), diesel::result::Error, _>(|c| {
            async {
                diesel::sql_query("DELETE FROM vaults WHERE vault_id = $1")
                    .bind::<Text, _>(&vault)
                    .execute(c)
                    .await?;
                Err(diesel::result::Error::RollbackTransaction)
            }
            .scope_boxed()
        })
        .await;
    assert!(rolled_back.is_err());
    assert_eq!(messages(&mut conn, &slot, &vault).await, Vec::<String>::new(), "a rolled-back delete");

    #[derive(diesel::QueryableByName)]
    struct Live {
        #[diesel(sql_type = diesel::sql_types::Jsonb)]
        row: serde_json::Value,
    }
    let live: Live = diesel::sql_query("SELECT to_jsonb(v) AS row FROM vaults v WHERE vault_id = $1")
        .bind::<Text, _>(&vault)
        .get_result(&mut *conn)
        .await
        .unwrap();
    assert_eq!(live.row["actor"], "apikey_a");
    write_as(&pg, &deleter, "DELETE FROM vaults WHERE vault_id = $1", &vault).await;
    let seen = messages(&mut conn, &slot, &vault).await;
    diesel::sql_query("SELECT pg_drop_replication_slot($1)")
        .bind::<Text, _>(&slot)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let content = seen[0].split_once("content:").unwrap().1;
    let message: serde_json::Value = serde_json::from_str(content).unwrap();
    let expected = serde_json::json!({
        "table": "vaults",
        "actor": deleter.actor,
        "request_id": deleter.request_id,
        "row": live.row,
    });
    assert_eq!(message, expected, "the row as it was, its last writer included");
    assert!(seen[0].starts_with("message: transactional: 1 prefix: agentd-history"), "{}", seen[0]);
}

#[tokio::test]
async fn every_tracked_table_logs_its_old_row_whole() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let mut conn = pg.diesel.get().await.unwrap();
    #[derive(diesel::QueryableByName)]
    struct Partial {
        #[diesel(sql_type = Text)]
        relname: String,
    }
    let partial: Vec<Partial> = diesel::sql_query(
        "SELECT c.relname::text AS relname FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid
          WHERE t.tgname LIKE '%\\_history\\_stamp' AND c.relreplident <> 'f'",
    )
    .load(&mut *conn)
    .await
    .unwrap();
    assert_eq!(partial.into_iter().map(|p| p.relname).collect::<Vec<_>>(), Vec::<String>::new());
}
