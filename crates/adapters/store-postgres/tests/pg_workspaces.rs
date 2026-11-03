//! Workspaces over live Postgres: org scope, the archived filter, and id-cursor
//! pages in both directions. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::{BTreeMap, HashMap};

use chrono::Duration;
use organizations::workspaces::types::{DataResidency, WorkspaceTag};
use organizations::workspaces::{StoreError, Workspace, WorkspacesStore};
use shared::{IdCursor, OrgId, Timestamp};
use store_postgres::{PgBackend, PgWorkspacesStore};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn store() -> PgWorkspacesStore {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgWorkspacesStore::new(PgBackend::connect(&url).await.unwrap())
}

fn org() -> OrgId {
    OrgId::new(format!("org_w{}", Timestamp::now().as_datetime().timestamp_micros()))
}

fn workspace(n: i64) -> Workspace {
    let created = Timestamp::from_datetime(chrono::Utc::now() - Duration::seconds(100 - n));
    Workspace {
        id: format!("wrkspc_{n:02}"),
        tag: WorkspaceTag::Workspace,
        name: format!("w{n}"),
        created_at: created,
        archived_at: None,
        display_color: "#6c5bb9".into(),
        data_residency: DataResidency::default(),
        tags: BTreeMap::new(),
        external_key_id: None,
        compartment_id: format!("wrkspc_{n:02}"),
    }
}

fn cursor(params: &[(&str, &str)]) -> IdCursor {
    let params: HashMap<String, String> = params.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
    IdCursor::from_params(&params).unwrap()
}

fn ids(rows: &[Workspace]) -> Vec<&str> {
    rows.iter().map(|w| w.id.as_str()).collect()
}

#[tokio::test]
async fn a_workspace_is_seen_only_in_its_org_and_replaced_whole() {
    if !enabled() {
        return;
    }
    let (store, org) = (store().await, org());
    let mut ws = workspace(1);
    store.insert(&org, &ws).await.unwrap();
    assert_eq!(store.get(&org, &ws.id).await.unwrap(), ws);
    assert!(matches!(store.get(&OrgId::new("org_other"), &ws.id).await, Err(StoreError::NotFound)));
    ws.name = "renamed".into();
    store.replace(&org, &ws).await.unwrap();
    assert_eq!(store.get(&org, &ws.id).await.unwrap().name, "renamed");
    assert!(matches!(store.replace(&OrgId::new("org_other"), &ws).await, Err(StoreError::NotFound)));
}

#[tokio::test]
async fn a_list_pages_newest_first_both_ways_and_hides_archived_ones() {
    if !enabled() {
        return;
    }
    let (store, org) = (store().await, org());
    for n in 1..=5 {
        store.insert(&org, &workspace(n)).await.unwrap();
    }
    let mut archived = workspace(3);
    archived.archived_at = Some(Timestamp::now());
    store.replace(&org, &archived).await.unwrap();

    let (all, more) = store.list(&org, true, &cursor(&[])).await.unwrap();
    assert_eq!(
        (ids(&all), more),
        (vec!["wrkspc_05", "wrkspc_04", "wrkspc_03", "wrkspc_02", "wrkspc_01"], false)
    );
    let (active, _) = store.list(&org, false, &cursor(&[])).await.unwrap();
    assert_eq!(ids(&active), ["wrkspc_05", "wrkspc_04", "wrkspc_02", "wrkspc_01"]);

    let (page, more) = store.list(&org, true, &cursor(&[("limit", "2")])).await.unwrap();
    assert_eq!((ids(&page), more), (vec!["wrkspc_05", "wrkspc_04"], true));
    let (page, more) =
        store.list(&org, true, &cursor(&[("limit", "2"), ("after_id", "wrkspc_04")])).await.unwrap();
    assert_eq!((ids(&page), more), (vec!["wrkspc_03", "wrkspc_02"], true));
    let (page, more) =
        store.list(&org, true, &cursor(&[("limit", "2"), ("before_id", "wrkspc_02")])).await.unwrap();
    assert_eq!((ids(&page), more), (vec!["wrkspc_04", "wrkspc_03"], true), "the rows nearest the cursor");
    let (page, more) =
        store.list(&org, true, &cursor(&[("limit", "2"), ("before_id", "wrkspc_04")])).await.unwrap();
    assert_eq!((ids(&page), more), (vec!["wrkspc_05"], false));
}
