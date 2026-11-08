//! Every store write leaves its writer on the connection it used, replacing
//! the previous writer's. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use chrono::SubsecRound;
use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;
use organizations::workspaces::types::{DataResidency, WorkspaceTag};
use organizations::workspaces::{Workspace, WorkspacesStore};
use shared::{with_writer, OrgId, Timestamp, Writer};
use store_postgres::{PgBackend, PgWorkspacesStore, PoolSettings};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

#[derive(diesel::QueryableByName)]
struct Setting {
    #[diesel(sql_type = Text)]
    actor: String,
    #[diesel(sql_type = Text)]
    request_id: String,
}

// One connection, so the read after each write lands on the connection the write used.
async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let settings = PoolSettings { max_size: 1, ..PoolSettings::default() };
    PgBackend::connect_with(&url, settings).await.unwrap()
}

async fn setting(backend: &PgBackend) -> Writer {
    let mut conn = backend.diesel.get().await.unwrap();
    let row: Setting = diesel::sql_query(
        "SELECT current_setting('agentd.actor', true) AS actor,
                current_setting('agentd.request_id', true) AS request_id",
    )
    .get_result(&mut *conn)
    .await
    .unwrap();
    Writer { actor: row.actor, request_id: row.request_id }
}

fn workspace() -> Workspace {
    let id = format!("wrkspc_writer{}", Timestamp::now().as_datetime().timestamp_micros());
    Workspace {
        id: id.clone(),
        tag: WorkspaceTag::Workspace,
        name: "writer".into(),
        created_at: Timestamp::from_datetime(chrono::Utc::now().trunc_subsecs(6)),
        archived_at: None,
        display_color: "#6c5bb9".into(),
        data_residency: DataResidency::default(),
        tags: BTreeMap::new(),
        external_key_id: None,
        compartment_id: id,
    }
}

#[tokio::test]
async fn each_write_leaves_its_own_writer_on_the_connection() {
    if !enabled() {
        return;
    }
    let backend = backend().await;
    let store = PgWorkspacesStore::new(backend.clone());
    let org = OrgId::new(format!("org_writer{}", Timestamp::now().as_datetime().timestamp_micros()));
    let mut ws = workspace();

    let first = Writer::fresh("apikey_a");
    with_writer(first.clone(), store.insert(&org, &ws)).await.unwrap();
    assert_eq!(setting(&backend).await, first);

    ws.name = "renamed".into();
    let second = Writer::fresh("apikey_b");
    with_writer(second.clone(), store.replace(&org, &ws)).await.unwrap();
    assert_eq!(setting(&backend).await, second);
}

#[tokio::test]
async fn a_write_outside_any_writer_scope_runs_as_system() {
    if !enabled() {
        return;
    }
    let backend = backend().await;
    let store = PgWorkspacesStore::new(backend.clone());
    let org = OrgId::new(format!("org_writersys{}", Timestamp::now().as_datetime().timestamp_micros()));

    store.insert(&org, &workspace()).await.unwrap();
    let seen = setting(&backend).await;
    assert_eq!(seen.actor, "system");
    assert!(seen.request_id.starts_with("req_"), "{}", seen.request_id);
}
