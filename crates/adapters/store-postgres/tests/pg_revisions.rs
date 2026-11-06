//! Environments keep every revision: writes through the `environments` view add rows to
//! `environment_revisions`, a delete only stamps the live row, and the table itself refuses
//! any other change. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use diesel::sql_types::{BigInt, Bool, Nullable, Text};
use diesel_async::RunQueryDsl;
use environments::types::{CreateEnvironmentRequest, UpdateEnvironmentRequest};
use environments::{Environment, EnvironmentsService, EnvironmentsStore};
use serde_json::json;
use shared::{with_writer, AuthContext, OrgId, Pagination, Timestamp, WorkspaceId, Writer};
use store_postgres::{PgBackend, PgEnvironmentsStore, PoolSettings};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

// One connection, so a write that skipped `write()` would show the previous writer.
async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let settings = PoolSettings { max_size: 1, ..PoolSettings::default() };
    PgBackend::connect_with(&url, settings).await.unwrap()
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn ctx() -> AuthContext {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    AuthContext {
        org: OrgId::new(format!("org_rev{}_{n}", Timestamp::now().as_datetime().timestamp_micros())),
        workspace: WorkspaceId::new("wrkspc_rev"),
        api_key_id: "apikey_rev".into(),
    }
}

fn svc(backend: &PgBackend) -> EnvironmentsService {
    EnvironmentsService::new(Arc::new(PgEnvironmentsStore::new(backend.clone())))
}

fn create_req(name: &str) -> CreateEnvironmentRequest {
    serde_json::from_value(json!({"name": name, "metadata": {"k": "v"}})).unwrap()
}

fn rename(name: &str) -> UpdateEnvironmentRequest {
    serde_json::from_value(json!({ "name": name })).unwrap()
}

#[derive(diesel::QueryableByName, Debug)]
struct Revision {
    #[diesel(sql_type = BigInt)]
    revision: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    superseded_by: Option<i64>,
    #[diesel(sql_type = Bool)]
    deleted: bool,
    #[diesel(sql_type = Nullable<Text>)]
    deleted_by: Option<String>,
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Nullable<Text>)]
    request_id: Option<String>,
    #[diesel(sql_type = Text)]
    doc: String,
    // Every column except the three stamps, as Postgres stores them.
    #[diesel(sql_type = Text)]
    body: String,
}

async fn revisions(backend: &PgBackend, ctx: &AuthContext, env: &Environment) -> Vec<Revision> {
    let mut conn = backend.diesel.get().await.unwrap();
    diesel::sql_query(
        "SELECT revision, superseded_by, deleted_at IS NOT NULL AS deleted, deleted_by, actor, request_id,
                doc::text AS doc, (to_jsonb(r) - 'superseded_by' - 'deleted_at' - 'deleted_by')::text AS body
           FROM environment_revisions r WHERE org_id = $1 AND environment_id = $2 ORDER BY revision",
    )
    .bind::<Text, _>(ctx.org.as_str())
    .bind::<Text, _>(env.id.as_str())
    .load(&mut *conn)
    .await
    .unwrap()
}

async fn exec(backend: &PgBackend, sql: &str) -> Result<usize, diesel::result::Error> {
    let mut conn = backend.diesel.get().await.unwrap();
    diesel::sql_query(sql).execute(&mut *conn).await
}

fn page() -> Pagination {
    Pagination::from_params(&std::collections::HashMap::new()).unwrap()
}

fn assert_written_by(row: &Revision, writer: &Writer) {
    assert_eq!(row.actor, writer.actor, "{row:?}");
    assert_eq!(row.request_id.as_deref(), Some(writer.request_id.as_str()), "{row:?}");
}

#[tokio::test]
async fn create_update_and_archive_each_add_one_row_by_their_writer() {
    if !enabled() {
        return;
    }
    let backend = backend().await;
    let svc = svc(&backend);
    let ctx = ctx();

    let creator = Writer::fresh("apikey_create");
    let env = with_writer(creator.clone(), svc.create(&ctx, create_req("one"))).await.unwrap();
    let rows = revisions(&backend, &ctx, &env).await;
    assert_eq!(rows.len(), 1);
    assert_written_by(&rows[0], &creator);
    assert_eq!((rows[0].superseded_by, rows[0].deleted), (None, false));
    let first_body = rows[0].body.clone();

    let updater = Writer::fresh("apikey_update");
    let updated =
        with_writer(updater.clone(), svc.update(&ctx, env.id.as_str(), rename("two"))).await.unwrap();
    let rows = revisions(&backend, &ctx, &env).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].superseded_by, Some(rows[1].revision));
    assert_eq!(rows[0].body, first_body, "the superseded row keeps its values");
    assert_written_by(&rows[0], &creator);
    assert_written_by(&rows[1], &updater);
    assert_eq!(rows[1].superseded_by, None);
    assert_eq!(svc.get(&ctx, env.id.as_str()).await.unwrap(), updated);

    let archiver = Writer::fresh("apikey_archive");
    let archived = with_writer(archiver.clone(), svc.archive(&ctx, env.id.as_str())).await.unwrap();
    let rows = revisions(&backend, &ctx, &env).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].superseded_by, Some(rows[2].revision));
    assert_written_by(&rows[2], &archiver);
    assert_eq!(svc.get(&ctx, env.id.as_str()).await.unwrap(), archived);
    assert_eq!(svc.list(&ctx, true, &page()).await.unwrap(), vec![archived]);
}

#[tokio::test]
async fn delete_stamps_the_live_row_adds_none_and_hides_the_environment() {
    if !enabled() {
        return;
    }
    let backend = backend().await;
    let svc = svc(&backend);
    let ctx = ctx();
    let env = svc.create(&ctx, create_req("gone")).await.unwrap();
    let kept = svc.create(&ctx, create_req("kept")).await.unwrap();
    let before = revisions(&backend, &ctx, &env).await;

    let deleter = Writer::fresh("apikey_delete");
    let returned = with_writer(deleter.clone(), svc.delete(&ctx, env.id.as_str())).await.unwrap();
    assert_eq!(returned, env, "DELETE ... RETURNING doc answers the live row");

    let rows = revisions(&backend, &ctx, &env).await;
    assert_eq!(rows.len(), 1, "a delete adds no row");
    assert!(rows[0].deleted);
    assert_eq!(rows[0].deleted_by.as_deref(), Some("apikey_delete"));
    assert_eq!(rows[0].body, before[0].body);

    assert_eq!(svc.get(&ctx, env.id.as_str()).await.unwrap_err().status, 404);
    assert_eq!(svc.list(&ctx, true, &page()).await.unwrap(), vec![kept.clone()]);
    assert_eq!(svc.delete(&ctx, env.id.as_str()).await.unwrap_err().status, 404);

    let delete_kept = format!(
        "DELETE FROM environments WHERE org_id = '{}' AND environment_id = '{}'",
        ctx.org.as_str(),
        kept.id.as_str()
    );
    assert_eq!(exec(&backend, &delete_kept).await.unwrap(), 1);
    assert_eq!(exec(&backend, &delete_kept).await.unwrap(), 0);
}

#[tokio::test]
async fn the_revisions_table_refuses_changes_except_the_stamps_once() {
    if !enabled() {
        return;
    }
    let backend = backend().await;
    let svc = svc(&backend);
    let ctx = ctx();
    let env = svc.create(&ctx, create_req("guarded")).await.unwrap();
    let row = format!(
        "FROM environment_revisions WHERE org_id = '{}' AND environment_id = '{}'",
        ctx.org.as_str(),
        env.id.as_str()
    );
    let update = |set: &str| {
        format!("UPDATE environment_revisions SET {set} WHERE (org_id, environment_id) IN (SELECT org_id, environment_id {row})")
    };
    let refused = |r: Result<usize, diesel::result::Error>| {
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("environment_revisions rows are"), "{msg}");
    };

    refused(exec(&backend, &format!("DELETE {row}")).await);
    refused(exec(&backend, &update("name = 'changed'")).await);
    refused(exec(&backend, &update("written_at = now() - interval '1 day'")).await);

    assert_eq!(exec(&backend, &update("deleted_at = now(), deleted_by = 'operator'")).await.unwrap(), 1);
    refused(exec(&backend, &update("deleted_at = now() + interval '1 second'")).await);
    refused(exec(&backend, &update("deleted_by = 'someone_else'")).await);

    assert_eq!(exec(&backend, &update("superseded_by = 9223372036854775806")).await.unwrap(), 1);
    refused(exec(&backend, &update("superseded_by = 9223372036854775807")).await);
    refused(exec(&backend, &update("superseded_by = null")).await);
    assert_eq!(revisions(&backend, &ctx, &env).await.len(), 1);
}

#[tokio::test]
async fn two_concurrent_updates_never_leave_two_live_rows() {
    if !enabled() {
        return;
    }
    let first = backend().await;
    let second = backend().await;
    let ctx = ctx();
    let env = svc(&first).create(&ctx, create_req("race")).await.unwrap();

    // The first update holds its transaction open while the second one runs.
    let mut conn = first.diesel.write().await.unwrap();
    diesel::sql_query("BEGIN").execute(&mut *conn).await.unwrap();
    let held = format!(
        "UPDATE environments SET name = 'first', doc = doc || '{{\"name\": \"first\"}}'
          WHERE org_id = '{}' AND environment_id = '{}'",
        ctx.org.as_str(),
        env.id.as_str()
    );
    assert_eq!(diesel::sql_query(&held).execute(&mut *conn).await.unwrap(), 1);

    let store = PgEnvironmentsStore::new(second.clone());
    let (org, mut racer) = (ctx.org.clone(), env.clone());
    racer.name = "second".into();
    let racing = tokio::spawn(async move { store.replace(&org, racer).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!racing.is_finished(), "the second update waits on the first's live row");

    diesel::sql_query("COMMIT").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(racing.await.unwrap().is_err(), "the second update fails on the live-row index");

    let rows = revisions(&first, &ctx, &env).await;
    assert_eq!(rows.iter().filter(|r| r.superseded_by.is_none() && !r.deleted).count(), 1);
    assert_eq!(rows.len(), 2);
    assert_eq!(svc(&first).get(&ctx, env.id.as_str()).await.unwrap().name, "first");
}

// The restore runbook: revision n comes back as a new live row written by `operator`,
// superseding the live row if there is one.
async fn restore(backend: &PgBackend, ctx: &AuthContext, env: &Environment, revision: i64) {
    let (org, id) = (ctx.org.as_str(), env.id.as_str());
    let mut conn = backend.diesel.get().await.unwrap();
    for sql in [
        "BEGIN".to_owned(),
        "SET LOCAL agentd.actor = 'operator'".to_owned(),
        "SET LOCAL agentd.request_id = 'restore'".to_owned(),
        "SELECT nextval(pg_get_serial_sequence('environment_revisions', 'revision'))".to_owned(),
        format!(
            "UPDATE environment_revisions
                SET superseded_by = currval(pg_get_serial_sequence('environment_revisions', 'revision'))
              WHERE org_id = '{org}' AND environment_id = '{id}'
                AND superseded_by IS NULL AND deleted_at IS NULL"
        ),
        format!(
            "INSERT INTO environment_revisions (org_id, environment_id, name, state, created_at, doc, revision)
             SELECT org_id, environment_id, name, state, created_at, doc,
                    currval(pg_get_serial_sequence('environment_revisions', 'revision'))
               FROM environment_revisions
              WHERE org_id = '{org}' AND environment_id = '{id}' AND revision = {revision}"
        ),
        "COMMIT".to_owned(),
    ] {
        diesel::sql_query(sql).execute(&mut *conn).await.unwrap();
    }
}

#[tokio::test]
async fn the_runbook_restores_a_deleted_environment_with_its_original_body() {
    if !enabled() {
        return;
    }
    let backend = backend().await;
    let svc = svc(&backend);
    let ctx = ctx();
    let original = svc.create(&ctx, create_req("original")).await.unwrap();
    let renamed = svc.update(&ctx, original.id.as_str(), rename("renamed")).await.unwrap();
    svc.delete(&ctx, original.id.as_str()).await.unwrap();
    let first = revisions(&backend, &ctx, &original).await[0].revision;

    restore(&backend, &ctx, &original, first).await;
    assert_eq!(svc.get(&ctx, original.id.as_str()).await.unwrap(), original);
    let rows = revisions(&backend, &ctx, &original).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].actor, "operator");
    assert_eq!(rows[2].doc, rows[0].doc);

    let second = rows[1].revision;
    restore(&backend, &ctx, &original, second).await;
    assert_eq!(svc.get(&ctx, original.id.as_str()).await.unwrap(), renamed);
    let rows = revisions(&backend, &ctx, &original).await;
    assert_eq!(rows[2].superseded_by, Some(rows[3].revision), "the live row is superseded");
    assert_eq!(rows.iter().filter(|r| r.superseded_by.is_none() && !r.deleted).count(), 1);
}

#[tokio::test]
async fn an_insert_through_the_view_takes_the_revisions_table_defaults() {
    if !enabled() {
        return;
    }
    let backend = backend().await;
    let ctx = ctx();
    let env = svc(&backend).create(&ctx, create_req("defaults")).await.unwrap();
    let insert = |id: &str| {
        format!(
            "INSERT INTO environments (org_id, environment_id, name, state, created_at, doc)
             SELECT org_id, '{id}', name, state, created_at, doc FROM environments
              WHERE org_id = '{}' AND environment_id = '{}'",
            ctx.org.as_str(),
            env.id.as_str()
        )
    };
    let mut copy = env.clone();

    exec(&backend, "SELECT set_config('agentd.actor', 'apikey_view', false), set_config('agentd.request_id', 'req_view', false)").await.unwrap();
    copy.id = environments::EnvironmentId::generate();
    assert_eq!(exec(&backend, &insert(copy.id.as_str())).await.unwrap(), 1);
    let rows = revisions(&backend, &ctx, &copy).await;
    assert_eq!(rows.len(), 1);
    assert_written_by(&rows[0], &Writer { actor: "apikey_view".into(), request_id: "req_view".into() });
    assert!(rows[0].revision > 0);
    assert_eq!((rows[0].superseded_by, rows[0].deleted), (None, false));

    exec(&backend, "RESET agentd.actor").await.unwrap();
    exec(&backend, "RESET agentd.request_id").await.unwrap();
    copy.id = environments::EnvironmentId::generate();
    exec(&backend, &insert(copy.id.as_str())).await.unwrap();
    let rows = revisions(&backend, &ctx, &copy).await;
    assert_eq!((rows[0].actor.as_str(), rows[0].request_id.as_deref()), ("unattributed", None));
}
