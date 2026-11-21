//! The history store (V17): one history table per tracked table, recorded
//! once per change, decoded from the feed's encoding, scrubbed on redaction,
//! and searchable by actor and request id across tables. Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use diesel::sql_types::{Jsonb, Text};
use diesel_async::RunQueryDsl;
use serde_json::{Value, json};
use shared::Timestamp;
use shared::ports::changes::Position;
use shared::ports::history::{HeldVersion, HistoryError, HistoryLog, HistoryOp};
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
    log.record("vaults", HistoryOp::Update, &at(20, 2), &vault(&id, "two", "apikey_b"), None).await.unwrap();
    log.record("vaults", HistoryOp::Insert, &at(10, 1), &vault(&id, "one", "apikey_a"), None).await.unwrap();
    log.record("vaults", HistoryOp::Insert, &at(10, 1), &vault(&id, "one", "apikey_a"), None).await.unwrap();
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
    log.record("api_keys", HistoryOp::Insert, &at(1, 1), &row, None).await.unwrap();
    let mut deleted = row.clone();
    deleted["key_digest"] = json!("\\x000102");
    log.record("api_keys", HistoryOp::Delete, &at(2, 2), &deleted, None).await.unwrap();
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
    let none = log.record("work_items", HistoryOp::Insert, &at(1, 1), &json!({}), None).await;
    assert!(matches!(none, Err(HistoryError::Unrecordable(_))), "{none:?}");
    let bad = log.record("vaults", HistoryOp::Insert, &at(1, 1), &json!({"doc": "{"}), None).await;
    assert!(matches!(bad, Err(HistoryError::Unrecordable(_))), "{bad:?}");
}

/// The memory version a row of `store` holds.
fn held<'a>(store: &'a str, version: &'a str) -> Option<HeldVersion<'a>> {
    Some(HeldVersion { workspace: "wrkspc_h", store, version })
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
async fn a_redaction_clears_its_version_and_the_heads_that_held_it_whenever_they_arrive() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let s = stamp();
    let (store, memory) = (format!("memstore_h{s}"), format!("mem_h{s}"));
    let (old, new) = (format!("memver_o{s}"), format!("memver_n{s}"));
    let (old_version, old_head) = memory_rows(&store, &memory, &old, Some("aaa"));
    let (new_version, new_head) = memory_rows(&store, &memory, &new, Some("bbb"));
    log.record("memory_versions", HistoryOp::Insert, &at(3, 1), &new_version, held(&store, &new))
        .await
        .unwrap();
    log.record("memories", HistoryOp::Update, &at(4, 1), &new_head, held(&store, &new)).await.unwrap();
    let versions =
        "SELECT jsonb_build_object('v', version_id, 'sha', doc->'content_sha256', 'path', doc->'path')
                      AS row FROM history.memory_versions WHERE store_id = $1 ORDER BY lsn";
    let heads = "SELECT jsonb_build_object('sha', doc->'content_sha256', 'path', path) AS row
                   FROM history.memories WHERE store_id = $1 ORDER BY lsn";
    let unredacted = json!({"v": new, "sha": "bbb", "path": "/a.md"});
    assert_eq!(rows(&pg, versions, &store).await, [unredacted], "a version not redacted keeps its content");

    let (redacted, _) = memory_rows(&store, &memory, &old, None);
    log.record("memory_versions", HistoryOp::Update, &at(5, 1), &redacted, held(&store, &old)).await.unwrap();
    // The head that held the old version arrives after the redaction, as another topic may deliver it.
    log.record("memories", HistoryOp::Insert, &at(2, 1), &old_head, held(&store, &old)).await.unwrap();
    // So does the version's own insert, which an unordered stream may hand over last.
    log.record("memory_versions", HistoryOp::Insert, &at(1, 1), &old_version, held(&store, &old))
        .await
        .unwrap();
    // And a head deleted for good after the redaction, its row as Postgres's to_jsonb gives it.
    let mut deleted = old_head.clone();
    deleted["doc"] = serde_json::from_str(old_head["doc"].as_str().unwrap()).unwrap();
    log.record("memories", HistoryOp::Delete, &at(6, 1), &deleted, held(&store, &old)).await.unwrap();
    let cleared = json!({"v": old, "sha": null, "path": null});
    assert_eq!(
        rows(&pg, versions, &store).await,
        vec![cleared.clone(), json!({"v": new, "sha": "bbb", "path": "/a.md"}), cleared]
    );
    let none = json!({"sha": null, "path": null});
    assert_eq!(
        rows(&pg, heads, &store).await,
        vec![none.clone(), json!({"sha": "bbb", "path": "/a.md"}), none]
    );
}

#[tokio::test]
async fn a_redaction_recorded_while_its_version_s_rows_are_leaves_none_of_its_content() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let kept = "SELECT jsonb_build_object('sha', doc->'content_sha256', 'size', doc->'content_size_bytes',
                                         'path', doc->'path') AS row
                  FROM history.memory_versions WHERE store_id = $1
                 UNION ALL
                SELECT jsonb_build_object('sha', doc->'content_sha256', 'size', doc->'content_size_bytes',
                                         'path', path, 'doc_path', doc->'path')
                  FROM history.memories WHERE store_id = $1";
    for round in 0..20 {
        let s = format!("{}{round}", stamp());
        let (store, memory, version) =
            (format!("memstore_r{s}"), format!("mem_r{s}"), format!("memver_r{s}"));
        let (inserted, head) = memory_rows(&store, &memory, &version, Some("aaa"));
        let (redacted, _) = memory_rows(&store, &memory, &version, None);
        let (first, second, third) = (at(1, 1), at(2, 1), at(3, 1));
        // The versions' recorder and the heads' record at once, as their streams do.
        let (a, b, c) = tokio::join!(
            log.record("memory_versions", HistoryOp::Update, &third, &redacted, held(&store, &version)),
            log.record("memory_versions", HistoryOp::Insert, &first, &inserted, held(&store, &version)),
            log.record("memories", HistoryOp::Insert, &second, &head, held(&store, &version)),
        );
        a.unwrap();
        b.unwrap();
        c.unwrap();
        let kept_rows = rows(&pg, kept, &store).await;
        let cleared = |r: &Value| r.as_object().is_some_and(|o| o.values().all(Value::is_null));
        assert_eq!(kept_rows.len(), 3, "round {round}");
        assert!(kept_rows.iter().all(cleared), "round {round}: {kept_rows:?}");
    }
}

#[tokio::test]
async fn a_scrub_that_fails_leaves_no_row_of_its_recording() {
    if !enabled() {
        return;
    }
    let (pg, log) = log().await;
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    // DATABASE_URL may be a URI or a keyword string; each takes the option its own way.
    let impatient = match url.starts_with("postgres://") || url.starts_with("postgresql://") {
        true => {
            let joiner = if url.contains('?') { '&' } else { '?' };
            format!("{url}{joiner}options=-c%20lock_timeout%3D200ms")
        }
        false => format!("{url} options='-c lock_timeout=200ms'"),
    };
    let impatient = PgBackend::connect(&impatient).await.unwrap();
    let impatient = PgHistoryLog::load(impatient).await.unwrap();
    let s = stamp();
    let (store, memory, version) = (format!("memstore_f{s}"), format!("mem_f{s}"), format!("memver_f{s}"));
    let (_, head) = memory_rows(&store, &memory, &version, Some("aaa"));
    let (redacted, _) = memory_rows(&store, &memory, &version, None);
    log.record("memories", HistoryOp::Insert, &at(1, 1), &head, held(&store, &version)).await.unwrap();
    // Another transaction holds the head's history row, so the scrub's second update times out.
    let mut holder = pg.diesel.get().await.unwrap();
    diesel::sql_query("BEGIN").execute(&mut *holder).await.unwrap();
    diesel::sql_query("SELECT 1 FROM history.memories WHERE store_id = $1 FOR UPDATE")
        .bind::<Text, _>(&store)
        .execute(&mut *holder)
        .await
        .unwrap();
    let failed = impatient
        .record("memory_versions", HistoryOp::Update, &at(2, 1), &redacted, held(&store, &version))
        .await;
    diesel::sql_query("ROLLBACK").execute(&mut *holder).await.unwrap();
    let Err(HistoryError::Backend(error)) = failed else { panic!("{failed:?}") };
    assert!(error.to_string().contains("lock timeout"), "{error}");
    let versions =
        "SELECT jsonb_build_object('v', version_id) AS row FROM history.memory_versions WHERE store_id = $1";
    assert_eq!(
        rows(&pg, versions, &store).await,
        Vec::<Value>::new(),
        "the redaction rolled back with its scrub"
    );
    let heads = "SELECT jsonb_build_object('sha', doc->'content_sha256', 'size', doc->'content_size_bytes',
                                          'path', path, 'doc_path', doc->'path') AS row
                   FROM history.memories WHERE store_id = $1";
    let kept = json!({"sha": "aaa", "size": 3, "path": "/a.md", "doc_path": "/a.md"});
    assert_eq!(rows(&pg, heads, &store).await, [kept], "the head keeps its content");
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
    log.record("vaults", HistoryOp::Insert, &at(5, 20), &row, None).await.unwrap();
    let skill = json!({"workspace_id": "wrkspc_h", "skill_id": format!("skill_s{s}"), "display_name": "s",
                       "created_at": "2026-11-16T10:00:00Z", "doc": "{}", "actor": actor, "request_id": format!("req_s{s}")});
    log.record("skills", HistoryOp::Insert, &at(6, 10), &skill, None).await.unwrap();
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

/// The two runbook tests each run the baseline, which copies every live row, so they hold this lock in turn.
const RUNBOOK_LOCK: &str = "SELECT pg_advisory_lock(7464)";
const RUNBOOK_UNLOCK: &str = "SELECT pg_advisory_unlock(7464)";

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
    let mut lock = pg.diesel.get().await.unwrap();
    diesel::sql_query(RUNBOOK_LOCK).execute(&mut *lock).await.unwrap();
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
    log.record("vaults", HistoryOp::Insert, &at(100, 1), &vault(&id, "one", "apikey_a"), None).await.unwrap();

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
    diesel::sql_query(RUNBOOK_UNLOCK).execute(&mut *lock).await.unwrap();
}

#[tokio::test]
async fn the_baseline_and_scrub_keep_no_content_of_a_version_redacted_after_its_memory_was_deleted() {
    if !enabled() {
        return;
    }
    let (pg, _) = log().await;
    let mut lock = pg.diesel.get().await.unwrap();
    diesel::sql_query(RUNBOOK_LOCK).execute(&mut *lock).await.unwrap();
    let blocks = runbook();
    let (memory, version) = (format!("mem_b{}", stamp()), format!("memver_b{}", stamp()));
    // A soft-deleted head keeps its last version's path and digest; that version is redacted after, so its
    // own row holds no content and only the head's copy needs the scrub.
    run(
        &pg,
        &format!(
            r#"INSERT INTO memory_versions (workspace_id, store_id, memory_id, version_id, has_blob, created_at, doc)
                    VALUES ('wrkspc_h', 'memstore_b', '{memory}', '{version}', false, now(),
                            '{{"id": "{version}", "content_sha256": null, "content_size_bytes": null, "path": null,
                               "redacted_at": "2026-11-17T00:00:00Z"}}');
               INSERT INTO memories (workspace_id, store_id, memory_id, path, head_version_id, deleted, created_at, doc)
                    VALUES ('wrkspc_h', 'memstore_b', '{memory}', '/a.md', '{version}', true, now(),
                            '{{"memory_version_id": "{version}", "content_sha256": "abc", "content_size_bytes": 3,
                               "path": "/a.md"}}')"#
        ),
    )
    .await;
    // Every field the scrub clears, of the head and of the version.
    const HEADS: &str = "SELECT jsonb_build_object('column_path', path, 'sha', doc->'content_sha256',
                                'size', doc->'content_size_bytes', 'path', doc->'path') AS row
                           FROM history.memories WHERE memory_id = $1 AND op = 'baseline'";
    const VERSIONS: &str = "SELECT jsonb_build_object('sha', doc->'content_sha256',
                                   'size', doc->'content_size_bytes', 'path', doc->'path') AS row
                              FROM history.memory_versions WHERE version_id = $1 AND op = 'baseline'";
    run(&pg, &block(&blocks, "do $$")).await;
    let copied = json!({"column_path": "/a.md", "sha": "abc", "size": 3, "path": "/a.md"});
    assert_eq!(rows(&pg, HEADS, &memory).await, [copied]);
    run(&pg, &block(&blocks, "update history.memories h")).await;
    let cleared = json!({"column_path": null, "sha": null, "size": null, "path": null});
    assert_eq!(rows(&pg, HEADS, &memory).await, [cleared]);
    assert_eq!(rows(&pg, VERSIONS, &version).await, [json!({"sha": null, "size": null, "path": null})]);
    diesel::sql_query(RUNBOOK_UNLOCK).execute(&mut *lock).await.unwrap();
}
