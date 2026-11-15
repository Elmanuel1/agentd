//! Webhook deliveries through HTTP: each lands in the commit of the change it
//! reports, with live's body (H-1, H-6, U3). Gated by `AGENTD_PG_TESTS=1` and
//! `AGENTD_AWS_ENDPOINT`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::{Auth, Rig, Streaming, KEY};
use serde_json::{json, Value};
use store_postgres::{PgSessionsStore, PgVaultsStore};
use webhooks::Webhooks;

/// The rig's app, its sessions and vaults sending to one endpoint of the rig's organization.
fn sending(rig: &Rig) -> axum::Router {
    let json = json!([{"id": "e1", "organization_id": rig.org, "url": "https://hooks.agentd.test/in", "secret": "whsec_c2VjcmV0"}]);
    let hooks = Arc::new(Webhooks::parse(&json.to_string()).unwrap());
    let mut stores = rig.stores.clone();
    stores.sessions = Arc::new(PgSessionsStore::new(rig.pg.clone()).with_webhooks(hooks.clone()));
    let vaults = Arc::new(PgVaultsStore::new(rig.pg.clone(), rig.secrets.clone()).with_webhooks(hooks));
    stores.vaults = vaults.clone();
    stores.vault_credentials = vaults;
    rig.replica(&stores, Streaming::default())
}

#[derive(diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type = diesel::sql_types::Text)]
    body: String,
}

/// The bodies of the deliveries about `id`, oldest first.
async fn sent(rig: &Rig, id: &str) -> Vec<Value> {
    let mut conn = rig.pg.diesel.get().await.unwrap();
    let q = diesel::sql_query(
        "SELECT body FROM webhook_deliveries WHERE body::jsonb->'data'->>'id' = $1 ORDER BY next_attempt_at, webhook_id",
    )
    .bind::<diesel::sql_types::Text, _>(id);
    let rows: Vec<Row> = diesel_async::RunQueryDsl::load(q, &mut *conn).await.unwrap();
    rows.into_iter().map(|r| serde_json::from_str(&r.body).unwrap()).collect()
}

fn kinds(bodies: &[Value]) -> Vec<&str> {
    bodies.iter().map(|b| b["data"]["type"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn an_archive_terminates_the_session_with_its_event_and_webhook() {
    // U3, H-1.
    let Some(rig) = Rig::new().await else { return };
    let app = sending(&rig);
    let env = rig.environment("self_hosted").await;
    let s = rig.session(&env, json!({})).await;
    let (st, archived) = common::send_to(
        &app,
        common::request("POST", &format!("/v1/sessions/{s}/archive"), Auth::Key(KEY), None),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{archived}");
    assert_eq!(archived["status"], "terminated");
    let (_, events) = rig.call("GET", &format!("/v1/sessions/{s}/events"), Auth::Key(KEY), None).await;
    assert_eq!(events["data"].as_array().unwrap().last().unwrap()["type"], "session.status_terminated");
    let bodies = sent(&rig, &s).await;
    assert_eq!(kinds(&bodies), ["session.status_terminated"]);
    let b = &bodies[0];
    assert_eq!(
        (b["type"].as_str(), b["data"]["organization_id"].as_str()),
        (Some("event"), Some(rig.org.as_str()))
    );
    assert_eq!(b["data"]["workspace_id"], rig.ws);
}

#[tokio::test]
async fn vault_and_credential_writes_send_live_s_lifecycle_events() {
    // H-6, B29: a vault's archive sends one event per credential it archives.
    let Some(rig) = Rig::new().await else { return };
    let app = sending(&rig);
    let call = |method: &'static str, path: String, body: Option<Value>| {
        let app = app.clone();
        async move { common::send_to(&app, common::request(method, &path, Auth::Key(KEY), body)).await }
    };
    let (_, vault) = call("POST", "/v1/vaults".into(), Some(json!({"display_name": "v"}))).await;
    let v = vault["id"].as_str().unwrap().to_owned();
    let mut creds = Vec::new();
    for n in 0..3 {
        let auth = json!({"type": "static_bearer", "mcp_server_url": format!("https://mcp{n}.agentd.test/mcp"), "token": "t"});
        let (st, c) = call(
            "POST",
            format!("/v1/vaults/{v}/credentials"),
            Some(json!({"display_name": "c", "auth": auth})),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{c}");
        creds.push(c["id"].as_str().unwrap().to_owned());
    }
    let (st, _) = call("DELETE", format!("/v1/vaults/{v}/credentials/{}", creds[0]), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call("POST", format!("/v1/vaults/{v}/archive"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(kinds(&sent(&rig, &v).await), ["vault.created", "vault.archived"]);
    let first = sent(&rig, &creds[0]).await;
    assert_eq!(kinds(&first), ["vault_credential.created", "vault_credential.deleted"]);
    assert_eq!(first[0]["data"]["vault_id"], v.as_str());
    for c in &creds[1..] {
        assert_eq!(kinds(&sent(&rig, c).await), ["vault_credential.created", "vault_credential.archived"]);
    }
    let (st, _) = call("DELETE", format!("/v1/vaults/{v}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(kinds(&sent(&rig, &v).await), ["vault.created", "vault.archived", "vault.deleted"]);
}

#[tokio::test]
async fn a_pending_delivery_is_sent_by_a_sender_started_after_it_was_written() {
    // H-4: the row is written in the change's commit; a replica that starts
    // later sends it when the feed hands it over, and records its end.
    use axum::routing::post;
    use shared::ports::changes::{Change, ChangeHandler, ChangeOp};
    let Some(rig) = Rig::new().await else { return };
    let got = Arc::new(parking_lot::Mutex::new(0));
    let seen = got.clone();
    let receiver = axum::Router::new().route(
        "/in",
        post(move || async move {
            *seen.lock() += 1;
            StatusCode::OK
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/in", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, receiver).await.ok() });
    let json = json!([{"id": "e1", "organization_id": rig.org, "url": url, "secret": "whsec_c2VjcmV0"}]);
    let hooks = Arc::new(Webhooks::parse(&json.to_string()).unwrap());
    let mut stores = rig.stores.clone();
    let vaults =
        Arc::new(PgVaultsStore::new(rig.pg.clone(), rig.secrets.clone()).with_webhooks(hooks.clone()));
    (stores.vaults, stores.vault_credentials) = (vaults.clone(), vaults);
    let app = rig.replica(&stores, Streaming::default());
    let (_, vault) = common::send_to(
        &app,
        common::request("POST", "/v1/vaults", Auth::Key(KEY), Some(json!({"display_name": "v"}))),
    )
    .await;
    let v = vault["id"].as_str().unwrap();

    #[derive(diesel::QueryableByName)]
    struct Pending {
        #[diesel(sql_type = diesel::sql_types::Jsonb)]
        row: Value,
    }
    let q = diesel::sql_query(
        "SELECT to_jsonb(d) AS row FROM webhook_deliveries d WHERE body::jsonb->'data'->>'id' = $1",
    )
    .bind::<diesel::sql_types::Text, _>(v);
    let mut conn = rig.pg.diesel.get().await.unwrap();
    let rows: Vec<Pending> = diesel_async::RunQueryDsl::load(q, &mut *conn).await.unwrap();
    assert_eq!(rows.len(), 1, "the delivery waits in Postgres");
    assert!(rows[0].row["delivered_at"].is_null());

    let marks = store_postgres::PgDeliveryMarks::new(rig.pg.clone());
    let sender = webhooks::WebhookSender::new(hooks, marks, std::time::Duration::from_millis(10)).unwrap();
    let change = Change {
        table: "webhook_deliveries".into(),
        op: ChangeOp::Insert,
        before: None,
        after: Some(rows[0].row.clone()),
    };
    sender.handle(&change).await.unwrap();
    assert_eq!(*got.lock(), 1);
    let rows: Vec<Pending> = diesel_async::RunQueryDsl::load(
        diesel::sql_query(
            "SELECT to_jsonb(d) AS row FROM webhook_deliveries d WHERE body::jsonb->'data'->>'id' = $1",
        )
        .bind::<diesel::sql_types::Text, _>(v),
        &mut *conn,
    )
    .await
    .unwrap();
    assert!(rows[0].row["delivered_at"].is_string(), "its end is recorded");
}
