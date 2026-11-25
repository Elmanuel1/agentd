//! Organizations over live Postgres: the rows `agentd organization create` makes, the foreign keys
//! every org-scoped row needs, and a delete that removes everything an organization owns. Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`; the outbox test also needs `AGENTD_CDC_TESTS=1`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use ::api_keys::{AdminKeysService, ApiKeysService, EnvironmentKeysService};
use ::organizations::registry::{OrganizationsStore, StoreError};
use diesel::sql_types::{BigInt, Text};
use diesel_async::RunQueryDsl;
use shared::ports::keys::{KeyPepper, PepperRing};
use shared::{OrgId, Timestamp};
use store_postgres::{
    PgAdminKeysStore, PgApiKeysStore, PgBackend, PgEnvironmentKeysStore, PgOrganizationsStore,
};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

fn tag() -> String {
    Timestamp::now().as_datetime().timestamp_nanos_opt().unwrap().to_string()
}

#[derive(diesel::QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

/// Run `sql` with `$1` = `a` and `$2` = `b` on a write checkout.
async fn exec(pg: &PgBackend, sql: &str, a: &str, b: &str) -> Result<usize, diesel::result::Error> {
    let mut conn = pg.diesel.write().await.unwrap();
    diesel::sql_query(sql).bind::<Text, _>(a).bind::<Text, _>(b).execute(&mut *conn).await
}

async fn count(pg: &PgBackend, sql: &str, a: &str) -> i64 {
    let mut conn = pg.diesel.get().await.unwrap();
    let row: Count = diesel::sql_query(sql).bind::<Text, _>(a).get_result(&mut *conn).await.unwrap();
    row.n
}

#[tokio::test]
async fn create_makes_one_row_and_refuses_an_existing_id() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let store = PgOrganizationsStore::new(pg.clone());
    let org = OrgId::new(format!("org_c{}", tag()));
    store.create(&org).await.unwrap();
    let again = store.create(&org).await.unwrap_err();
    assert_eq!(again.to_string(), format!("organization {} already exists", org.as_str()));
    assert_eq!(
        count(&pg, "SELECT count(*) AS n FROM organizations WHERE org_id = $1", org.as_str()).await,
        1
    );
}

#[tokio::test]
async fn every_org_scoped_write_needs_an_existing_organization() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let org = format!("org_none{}", tag());
    let writes = [
        "INSERT INTO workspaces (org_id, workspace_id, created_at, doc) VALUES ($1, $2, now(), '{}')",
        "INSERT INTO environments (org_id, environment_id, name, state, created_at, doc)
         VALUES ($1, $2, 'e', 'active', now(), '{}')",
        "INSERT INTO admin_keys (admin_key_id, org_id, name, key_digest, partial_key_hint, created_at)
         VALUES ($2, $1, 'k', decode(md5($2) || md5($2), 'hex'), 'h', now())",
        "INSERT INTO api_keys (api_key_id, org_id, workspace_id, name, key_digest, partial_key_hint, created_at)
         VALUES ($2, $1, 'wrkspc_x', 'k', decode(md5($2) || md5($2), 'hex'), 'h', now())",
        "INSERT INTO environment_keys (environment_key_id, org_id, workspace_id, environment_id, name, key_digest,
           partial_key_hint, created_at)
         VALUES ($2, $1, 'wrkspc_x', 'env_x', 'k', decode(md5($2) || md5($2), 'hex'), 'h', now())",
        "INSERT INTO work_items (workspace_id, session_id, org_id, environment_id, queue, state, metadata, created_at)
         VALUES ('wrkspc_x', $2, $1, 'env_x', 'q', 'queued', '{}', now())",
    ];
    for (n, sql) in writes.iter().enumerate() {
        let err = exec(&pg, sql, &org, &format!("id_{n}_{}", tag())).await.unwrap_err();
        assert!(err.to_string().contains("foreign key"), "{sql}: {err}");
    }
}

#[tokio::test]
async fn a_key_for_an_unknown_organization_fails_naming_it() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let peppers = Arc::new(PepperRing::single(KeyPepper::new(&[7; 32])));
    let org = format!("org_unknown{}", tag());
    let expected = format!("key store: no organization {org}");
    let keys = ApiKeysService::new(Arc::new(PgApiKeysStore::new(pg.clone())), peppers.clone());
    assert_eq!(keys.create(&org, "wrkspc_x", "ci").await.unwrap_err().to_string(), expected);
    let admin = AdminKeysService::new(Arc::new(PgAdminKeysStore::new(pg.clone())), peppers.clone());
    assert_eq!(admin.create(&org, "ci").await.unwrap_err().to_string(), expected);
    let env = EnvironmentKeysService::new(Arc::new(PgEnvironmentKeysStore::new(pg.clone())), peppers);
    assert_eq!(env.create(&org, "wrkspc_x", "env_x", "ci").await.unwrap_err().to_string(), expected);
}

#[tokio::test]
async fn deleting_an_unknown_organization_deletes_nothing() {
    if !enabled() {
        return;
    }
    let store = PgOrganizationsStore::new(backend().await);
    let org = OrgId::new(format!("org_gone{}", tag()));
    assert!(matches!(store.delete(&org).await, Err(StoreError::NotFound(id)) if id == org.as_str()));
}

/// An organization with a workspace row, a key naming a workspace with no row, and in those two
/// workspaces a credential, a file, a skill version, a memory version with a blob and one without,
/// a live and an archived deployment and dream, and an agent.
async fn seeded(pg: &PgBackend) -> (OrgId, String, String) {
    let t = tag();
    let (org, ws, keyed) = (format!("org_d{t}"), format!("wrkspc_d{t}"), format!("wrkspc_k{t}"));
    PgOrganizationsStore::new(pg.clone()).create(&OrgId::new(&org)).await.unwrap();
    let rows = [
        "INSERT INTO workspaces (org_id, workspace_id, created_at, doc) VALUES ($1, $2, now(), '{}')",
        "INSERT INTO api_keys (api_key_id, org_id, workspace_id, name, key_digest, partial_key_hint, created_at)
         VALUES ('apikey_' || $2, $1, $2, 'k', decode(md5($2) || md5($2), 'hex'), 'h', now())",
    ];
    exec(pg, rows[0], &org, &ws).await.unwrap();
    exec(pg, rows[1], &org, &keyed).await.unwrap();
    let in_workspace = [
        "INSERT INTO credentials (workspace_id, vault_id, credential_id, display_name, has_secret, created_at, doc)
         VALUES ($1, 'vlt_a', 'vcrd_a', 'c', true, now(), $2::jsonb)",
        "INSERT INTO files (workspace_id, file_id, filename, size_bytes, created_at, doc)
         VALUES ($1, 'file_a', 'f', 1, now(), $2::jsonb)",
        "INSERT INTO skill_versions (workspace_id, skill_id, version, created_at, doc)
         VALUES ($1, 'skill_a', 1, now(), $2::jsonb || '{\"id\": \"skver_a\"}')",
        "INSERT INTO memory_versions (workspace_id, store_id, memory_id, version_id, has_blob, created_at, doc)
         VALUES ($1, 'memstore_a', 'mem_a', 'memver_a', true, now(), $2::jsonb),
                ($1, 'memstore_a', 'mem_a', 'memver_b', false, now(), $2::jsonb)",
        "INSERT INTO deployments (workspace_id, deployment_id, name, status, created_at, doc)
         VALUES ($1, 'depl_a', 'd', 'active', now(), $2::jsonb), ($1, 'depl_b', 'd', 'archived', now(), $2::jsonb)",
        "INSERT INTO dreams (workspace_id, dream_id, name, status, created_at, doc)
         VALUES ($1, 'drm_a', 'd', 'active', now(), $2::jsonb), ($1, 'drm_b', 'd', 'archived', now(), $2::jsonb)",
        "INSERT INTO agents (workspace_id, agent_id, version, name, created_at, doc)
         VALUES ($1, 'agent_a', 1, 'a', now(), $2::jsonb)",
    ];
    for sql in in_workspace {
        exec(pg, sql, &ws, "{}").await.unwrap();
    }
    exec(pg, in_workspace[0], &keyed, "{}").await.unwrap();
    (OrgId::new(org), ws, keyed)
}

#[tokio::test]
async fn a_delete_removes_the_organization_and_everything_it_owns() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let (org, ws, keyed) = seeded(&pg).await;
    let removed = PgOrganizationsStore::new(pg.clone()).delete(&org).await.unwrap();

    let mut workspaces = removed.workspaces.clone();
    workspaces.sort();
    let mut expected = vec![ws.clone(), keyed.clone()];
    expected.sort();
    assert_eq!(workspaces, expected);
    let mut secrets: Vec<String> = removed.secrets.iter().map(ToString::to_string).collect();
    secrets.sort();
    let mut want = vec![format!("{ws}/vlt_a/vcrd_a"), format!("{keyed}/vlt_a/vcrd_a")];
    want.sort();
    assert_eq!(secrets, want);
    let mut blobs = removed.blobs.clone();
    blobs.sort();
    assert_eq!(
        blobs,
        [
            format!("memory/{ws}/memstore_a/memver_a"),
            format!("skills/{ws}/skill_a/skver_a"),
            format!("{ws}/file_a")
        ]
    );
    let dreams: Vec<String> = removed.dreams.iter().map(|d| d.key()).collect();
    assert_eq!(dreams, ["dream/drm_a"]);

    for table in
        ["credentials", "files", "skill_versions", "memory_versions", "deployments", "dreams", "agents"]
    {
        let sql = format!("SELECT count(*) AS n FROM {table} WHERE workspace_id IN ($1, '{keyed}')");
        assert_eq!(count(&pg, &sql, &ws).await, 0, "{table}");
    }
    for table in ["organizations", "workspaces", "api_keys"] {
        let sql = format!("SELECT count(*) AS n FROM {table} WHERE org_id = $1");
        assert_eq!(count(&pg, &sql, org.as_str()).await, 0, "{table}");
    }
}

#[tokio::test]
async fn a_delete_writes_a_schedule_delete_for_each_deployment_not_archived() {
    if !enabled() || std::env::var("AGENTD_CDC_TESTS").as_deref() != Ok("1") {
        return;
    }
    let pg = backend().await;
    let slot = format!("orgdel{}", tag());
    exec(
        &pg,
        "SELECT 1 FROM pg_create_logical_replication_slot($1, 'test_decoding') WHERE $2 = $2",
        &slot,
        "",
    )
    .await
    .unwrap();
    let (org, _, _) = seeded(&pg).await;
    PgOrganizationsStore::new(pg.clone()).delete(&org).await.unwrap();
    let inserts = "SELECT count(*) AS n FROM pg_logical_slot_peek_changes($1, NULL, NULL)
                   WHERE data LIKE 'table public.schedule_outbox: INSERT:%'";
    let live = count(&pg, &format!("{inserts} AND data LIKE '%deployment/depl_a%'"), &slot).await;
    let archived = count(&pg, &format!("{inserts} AND data LIKE '%deployment/depl_b%'"), &slot).await;
    exec(&pg, "SELECT 1 FROM pg_drop_replication_slot($1) WHERE $2 = $2", &slot, "").await.unwrap();
    assert_eq!((live, archived), (1, 0));
}

#[tokio::test]
async fn an_organization_delete_announces_the_organization_and_each_cascaded_environment() {
    if !enabled() || std::env::var("AGENTD_CDC_TESTS").as_deref() != Ok("1") {
        return;
    }
    let pg = backend().await;
    let slot = format!("orgfeed{}", tag());
    exec(
        &pg,
        "SELECT 1 FROM pg_create_logical_replication_slot($1, 'test_decoding') WHERE $2 = $2",
        &slot,
        "",
    )
    .await
    .unwrap();
    let org = OrgId::new(format!("org_feed{}", tag()));
    PgOrganizationsStore::new(pg.clone()).create(&org).await.unwrap();
    let env = "INSERT INTO environments (org_id, environment_id, name, state, created_at, doc)
               VALUES ($1, $2, 'e', 'active', now(), '{}')";
    exec(&pg, env, org.as_str(), &format!("env_feed{}", tag())).await.unwrap();
    PgOrganizationsStore::new(pg.clone()).delete(&org).await.unwrap();
    let announced = |table: &str| {
        format!(
            "SELECT count(*) AS n FROM pg_logical_slot_peek_changes($1, NULL, NULL)
             WHERE data LIKE 'message:%agentd-history%' AND data LIKE '%\"table\": \"{table}\"%'
               AND data LIKE '%{}%'",
            org.as_str()
        )
    };
    let orgs = count(&pg, &announced("organizations"), &slot).await;
    let envs = count(&pg, &announced("environments"), &slot).await;
    exec(&pg, "SELECT 1 FROM pg_drop_replication_slot($1) WHERE $2 = $2", &slot, "").await.unwrap();
    assert_eq!((orgs, envs), (1, 1));
}
