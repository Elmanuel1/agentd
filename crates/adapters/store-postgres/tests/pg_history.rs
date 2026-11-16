//! The history store (V17): one history table per tracked table, recorded
//! once per change, decoded from the feed's encoding, scrubbed on redaction,
//! and searchable by actor and request id across tables. Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use diesel::sql_types::{Jsonb, Text};
use diesel_async::RunQueryDsl;
use serde_json::{json, Value};
use shared::ports::changes::Position;
use shared::ports::history::{HistoryError, HistoryLog, HistoryOp};
use shared::Timestamp;
use store_postgres::{PgBackend, PgHistoryLog};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn log() -> (PgBackend, PgHistoryLog) {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let pg = PgBackend::connect(&url).await.unwrap();
    (pg.clone(), PgHistoryLog::load(pg).await.unwrap())
}

fn stamp() -> String {
    Timestamp::now().as_datetime().timestamp_nanos_opt().unwrap().to_string()
}

fn at(lsn: u64, seconds: i64) -> Position {
    let committed = chrono::DateTime::from_timestamp(1_790_000_000 + seconds, 0).unwrap();
    Position { lsn, tx_id: Some(7), committed_at: Timestamp::from_datetime(committed) }
}

#[derive(diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type = Jsonb)]
    row: Value,
}

/// `sql`'s rows as JSON, `$1` bound to `id`.
async fn rows(pg: &PgBackend, sql: &str, id: &str) -> Vec<Value> {
    let mut conn = pg.diesel.get().await.unwrap();
    let rows: Vec<Row> = diesel::sql_query(sql).bind::<Text, _>(id).load(&mut *conn).await.unwrap();
    rows.into_iter().map(|r| r.row).collect()
}

fn vault(id: &str, name: &str, actor: &str) -> Value {
    json!({"workspace_id": "wrkspc_h", "vault_id": id, "display_name": name, "created_at": "2026-11-16T10:00:00Z",
           "doc": format!("{{\"display_name\":\"{name}\"}}"), "actor": actor, "request_id": format!("req_{id}")})
}

#[tokio::test]
async fn a_change_recorded_twice_is_kept_once_and_changes_read_in_lsn_order() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let id = format!("vlt_h{}", stamp());
    log.record("vaults", HistoryOp::Update, &at(20, 2), &vault(&id, "two", "apikey_b")).await.unwrap();
    log.record("vaults", HistoryOp::Insert, &at(10, 1), &vault(&id, "one", "apikey_a")).await.unwrap();
    log.record("vaults", HistoryOp::Insert, &at(10, 1), &vault(&id, "one", "apikey_a")).await.unwrap();
    let got = rows(
        &pg,
        "SELECT jsonb_build_object('op', op, 'doc', doc, 'actor', actor, 'lsn', lsn::text) AS row
           FROM history.vaults WHERE vault_id = $1 ORDER BY lsn",
        &id,
    )
    .await;
    assert_eq!(
        got,
        vec![
            json!({"op": "insert", "doc": {"display_name": "one"}, "actor": "apikey_a", "lsn": "0/A"}),
            json!({"op": "update", "doc": {"display_name": "two"}, "actor": "apikey_b", "lsn": "0/14"}),
        ]
    );
}

#[tokio::test]
async fn the_feed_s_jsonb_text_and_base64_bytea_store_typed_and_a_delete_row_stores_as_postgres_gave_it() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let id = format!("apikey_h{}", stamp());
    let row = json!({"api_key_id": id, "org_id": "org_h", "workspace_id": "wrkspc_h", "name": "k",
                     "key_digest": "AAEC", "partial_key_hint": "sk-...h", "created_at": "2026-11-16T10:00:00Z",
                     "revoked_at": null, "actor": "operator", "request_id": null});
    log.record("api_keys", HistoryOp::Insert, &at(1, 1), &row).await.unwrap();
    let mut deleted = row.clone();
    deleted["key_digest"] = json!("\\x000102");
    log.record("api_keys", HistoryOp::Delete, &at(2, 2), &deleted).await.unwrap();
    let got = rows(
        &pg,
        "SELECT jsonb_build_object('digest', encode(key_digest, 'hex'), 'op', op) AS row
           FROM history.api_keys WHERE api_key_id = $1 ORDER BY lsn",
        &id,
    )
    .await;
    assert_eq!(
        got,
        vec![json!({"digest": "000102", "op": "insert"}), json!({"digest": "000102", "op": "delete"})]
    );
}

#[tokio::test]
async fn a_table_with_no_history_or_a_row_that_does_not_decode_is_unrecordable() {
    if !enabled() {
        return;
    }
    let (_, log) = log().await;
    let none = log.record("work_items", HistoryOp::Insert, &at(1, 1), &json!({})).await;
    assert!(matches!(none, Err(HistoryError::Unrecordable(_))), "{none:?}");
    let bad = log.record("vaults", HistoryOp::Insert, &at(1, 1), &json!({"doc": "{"})).await;
    assert!(matches!(bad, Err(HistoryError::Unrecordable(_))), "{bad:?}");
}

/// A memory version's row and its head's, as the feed sends them.
fn memory_rows(store: &str, memory: &str, version: &str, sha: Option<&str>) -> (Value, Value) {
    let doc =
        json!({"content_sha256": sha, "content_size_bytes": sha.map(|_| 3), "path": sha.map(|_| "/a.md"),
                     "redacted_at": sha.map_or(json!("2026-11-16T10:00:00Z"), |_| Value::Null)})
        .to_string();
    let common = json!({"workspace_id": "wrkspc_h", "store_id": store, "memory_id": memory,
                        "created_at": "2026-11-16T10:00:00Z", "doc": doc});
    let mut version_row = common.clone();
    version_row["version_id"] = json!(version);
    version_row["has_blob"] = json!(sha.is_some());
    let mut head = common;
    head["head_version_id"] = json!(version);
    head["path"] = json!("/a.md");
    head["deleted"] = json!(false);
    (version_row, head)
}

#[tokio::test]
async fn a_scrub_clears_a_redacted_version_and_the_heads_that_held_it_whenever_they_arrive() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let s = stamp();
    let (store, memory) = (format!("memstore_h{s}"), format!("mem_h{s}"));
    let (old, new) = (format!("memver_o{s}"), format!("memver_n{s}"));
    let (old_version, old_head) = memory_rows(&store, &memory, &old, Some("aaa"));
    let (new_version, new_head) = memory_rows(&store, &memory, &new, Some("bbb"));
    log.record("memory_versions", HistoryOp::Insert, &at(1, 1), &old_version).await.unwrap();
    log.record("memory_versions", HistoryOp::Insert, &at(3, 1), &new_version).await.unwrap();
    log.record("memories", HistoryOp::Update, &at(4, 1), &new_head).await.unwrap();
    let versions =
        "SELECT jsonb_build_object('v', version_id, 'sha', doc->'content_sha256', 'path', doc->'path')
                      AS row FROM history.memory_versions WHERE store_id = $1 ORDER BY lsn";
    let heads = "SELECT jsonb_build_object('sha', doc->'content_sha256', 'path', path) AS row
                   FROM history.memories WHERE store_id = $1 ORDER BY lsn";
    log.scrub_memory_content("wrkspc_h", &store, &old).await.unwrap();
    assert_eq!(
        rows(&pg, versions, &store).await[0],
        json!({"v": old, "sha": "aaa", "path": "/a.md"}),
        "not redacted"
    );

    let (redacted, _) = memory_rows(&store, &memory, &old, None);
    log.record("memory_versions", HistoryOp::Update, &at(5, 1), &redacted).await.unwrap();
    log.scrub_memory_content("wrkspc_h", &store, &old).await.unwrap();
    // The head that held the old version arrives after the redaction, as another topic may deliver it.
    log.record("memories", HistoryOp::Insert, &at(2, 1), &old_head).await.unwrap();
    log.scrub_memory_content("wrkspc_h", &store, &old).await.unwrap();
    let cleared = json!({"v": old, "sha": null, "path": null});
    assert_eq!(
        rows(&pg, versions, &store).await,
        vec![cleared.clone(), json!({"v": new, "sha": "bbb", "path": "/a.md"}), cleared]
    );
    assert_eq!(
        rows(&pg, heads, &store).await,
        vec![json!({"sha": null, "path": null}), json!({"sha": "bbb", "path": "/a.md"})]
    );
}

#[derive(diesel::QueryableByName)]
struct Name {
    #[diesel(sql_type = Text)]
    name: String,
}

async fn names(pg: &PgBackend, sql: &str) -> Vec<String> {
    let mut conn = pg.diesel.get().await.unwrap();
    let rows: Vec<Name> = diesel::sql_query(sql).load(&mut *conn).await.unwrap();
    rows.into_iter().map(|r| r.name).collect()
}

#[tokio::test]
async fn every_tracked_table_has_a_history_table_with_its_columns_and_types() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let tracked = names(
        &pg,
        "SELECT c.relname::text AS name FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid
          WHERE t.tgname = c.relname || '_history_stamp' ORDER BY c.relname COLLATE \"C\"",
    )
    .await;
    assert_eq!(tracked.len(), 18, "{tracked:?}");
    assert_eq!(log.tables(), tracked);
    let columns =
        "SELECT format('%s.%s %s', c.relname, a.attname, format_type(a.atttypid, a.atttypmod)) AS name
                     FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid
                     JOIN pg_namespace n ON n.oid = c.relnamespace
                    WHERE n.nspname = '{schema}' AND c.relkind = 'r' AND a.attnum > 0 AND NOT a.attisdropped
                      AND c.relname = ANY('{tables}') ORDER BY 1";
    let set = format!("{{{}}}", tracked.join(","));
    let live = names(&pg, &columns.replace("{schema}", "public").replace("{tables}", &set)).await;
    let kept = names(&pg, &columns.replace("{schema}", "history").replace("{tables}", &set)).await;
    let own = [".lsn pg_lsn", ".tx_id bigint", ".op text", ".committed_at timestamp with time zone"];
    let kept: Vec<String> = kept.into_iter().filter(|c| !own.iter().any(|o| c.ends_with(o))).collect();
    assert_eq!(kept, live);
    let in_view = names(
        &pg,
        "SELECT table_name::text AS name FROM information_schema.view_table_usage
          WHERE view_schema = 'history' AND view_name = 'changes' ORDER BY table_name COLLATE \"C\"",
    )
    .await;
    assert_eq!(in_view, tracked);
}

#[tokio::test]
async fn one_actor_s_changes_and_one_request_s_read_across_tables_by_index() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let s = stamp();
    let actor = format!("apikey_s{s}");
    let mut row = vault(&format!("vlt_s{s}"), "v", &actor);
    row["request_id"] = json!(format!("req_s{s}"));
    log.record("vaults", HistoryOp::Insert, &at(5, 20), &row).await.unwrap();
    let skill = json!({"workspace_id": "wrkspc_h", "skill_id": format!("skill_s{s}"), "display_name": "s",
                       "created_at": "2026-11-16T10:00:00Z", "doc": "{}", "actor": actor, "request_id": format!("req_s{s}")});
    log.record("skills", HistoryOp::Insert, &at(6, 10), &skill).await.unwrap();
    let by_actor = "SELECT jsonb_build_object('t', table_name, 'key', row_key) AS row FROM history.changes
                     WHERE actor = $1 AND committed_at >= to_timestamp(1790000000) ORDER BY committed_at";
    let got = rows(&pg, by_actor, &actor).await;
    let tables: Vec<&Value> = got.iter().map(|r| &r["t"]).collect();
    assert_eq!(tables, [&json!("skills"), &json!("vaults")]);
    assert_eq!(got[1]["key"], json!({"workspace_id": "wrkspc_h", "vault_id": format!("vlt_s{s}")}));
    let by_request = "SELECT jsonb_build_object('t', table_name) AS row FROM history.changes
                       WHERE request_id = $1 ORDER BY committed_at";
    assert_eq!(rows(&pg, by_request, &format!("req_s{s}")).await.len(), 2);

    let mut conn = pg.diesel.get().await.unwrap();
    for (sql, index) in [(by_actor, "_actor_idx"), (by_request, "_request_idx")] {
        diesel::sql_query("SET enable_seqscan = off").execute(&mut *conn).await.unwrap();
        #[derive(diesel::QueryableByName)]
        struct Plan {
            #[diesel(sql_type = Text, column_name = "QUERY PLAN")]
            line: String,
        }
        let plan: Vec<Plan> = diesel::sql_query(format!("EXPLAIN {sql}"))
            .bind::<Text, _>(&actor)
            .load(&mut *conn)
            .await
            .unwrap();
        diesel::sql_query("RESET enable_seqscan").execute(&mut *conn).await.unwrap();
        let plan: Vec<String> = plan.into_iter().map(|p| p.line).collect();
        let scans = plan.iter().filter(|l| l.contains("Index") && l.contains(index)).count();
        assert_eq!(scans, 18, "{}", plan.join("\n"));
    }
}

/// The SQL blocks of `docs/history.md`, as written.
fn runbook() -> Vec<String> {
    let doc =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../docs/history.md")).unwrap();
    doc.split("```sql\n").skip(1).map(|b| b.split("```").next().unwrap().to_owned()).collect()
}

fn block(blocks: &[String], marker: &str) -> String {
    blocks.iter().find(|b| b.contains(marker)).unwrap_or_else(|| panic!("no block with {marker}")).clone()
}

#[derive(diesel::QueryableByName)]
struct Live {
    #[diesel(sql_type = Text)]
    display_name: String,
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<Text>)]
    request_id: Option<String>,
}

async fn live(pg: &PgBackend, vault: &str) -> Option<(String, String, Option<String>)> {
    let mut conn = pg.diesel.get().await.unwrap();
    let rows: Vec<Live> =
        diesel::sql_query("SELECT display_name, actor, request_id FROM vaults WHERE vault_id = $1")
            .bind::<Text, _>(vault)
            .load(&mut *conn)
            .await
            .unwrap();
    rows.into_iter().next().map(|r| (r.display_name, r.actor, r.request_id))
}

/// Run a runbook block on its own connection, as an operator's psql session would.
async fn run(pg: &PgBackend, sql: &str) {
    use diesel_async::SimpleAsyncConnection;
    let mut conn = pg.diesel.get().await.unwrap();
    conn.batch_execute(sql).await.unwrap();
    conn.batch_execute("RESET agentd.actor; RESET agentd.request_id").await.unwrap();
}

#[tokio::test]
async fn the_runbook_s_rollbacks_and_baseline_run_as_documented() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let blocks = runbook();
    let id = format!("vlt_r{}", stamp());
    let fill = |sql: String, lsn: &str| {
        sql.replace("<workspace_id>", "wrkspc_h")
            .replace("<vault_id>", &id)
            .replace("<lsn>", lsn)
            .replace("<ticket>", "req_ticket")
    };
    run(
        &pg,
        &format!(
            "INSERT INTO vaults (workspace_id, vault_id, display_name, created_at, doc)
                       VALUES ('wrkspc_h', '{id}', 'two', now(), '{{}}')"
        ),
    )
    .await;
    log.record("vaults", HistoryOp::Insert, &at(100, 1), &vault(&id, "one", "apikey_a")).await.unwrap();

    run(&pg, &fill(block(&blocks, "update vaults v"), "0/64")).await;
    assert_eq!(live(&pg, &id).await, Some(("one".into(), "operator".into(), Some("req_ticket".into()))));

    run(&pg, &format!("DELETE FROM vaults WHERE vault_id = '{id}'")).await;
    assert_eq!(live(&pg, &id).await, None);
    run(&pg, &fill(block(&blocks, "insert into vaults"), "0/64")).await;
    assert_eq!(live(&pg, &id).await, Some(("one".into(), "operator".into(), Some("req_ticket".into()))));

    run(&pg, &block(&blocks, "do $$")).await;
    run(&pg, &block(&blocks, "do $$")).await;
    let baseline = rows(
        &pg,
        "SELECT jsonb_build_object('name', display_name, 'actor', actor) AS row FROM history.vaults
          WHERE vault_id = $1 AND op = 'baseline'",
        &id,
    )
    .await;
    assert!(
        !baseline.is_empty() && baseline.iter().all(|r| r == &json!({"name": "one", "actor": "operator"}))
    );
    for sql in
        [block(&blocks, "where actor = '<actor>'"), block(&blocks, "where request_id = '<request_id>'")]
    {
        let sql = sql
            .replace("<actor>", "operator")
            .replace("<from>", "2026-01-01")
            .replace("<to>", "2100-01-01")
            .replace("<request_id>", "req_ticket");
        run(&pg, &sql).await;
    }
    run(&pg, &fill(block(&blocks, "order by lsn;"), "0/64")).await;
}
