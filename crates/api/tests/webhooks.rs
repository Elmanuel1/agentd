//! Webhook deliveries through HTTP: each lands in the commit of the change it
//! reports, with live's body (H-1, H-6, U3); and their sending from the rows
//! ([`queue`]). Gated by `AGENTD_PG_TESTS=1` and `AGENTD_AWS_ENDPOINT`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;
#[path = "webhooks/queue.rs"]
mod queue;

use std::sync::Arc;

use axum::http::StatusCode;
use common::{Auth, KEY, Rig, Streaming};
use serde_json::{Value, json};
use webhooks::Webhooks;

/// The rig's app, its sessions and vaults sending to one endpoint of the rig's organization.
fn sending(rig: &Rig) -> axum::Router {
    let json = json!([{"id": "e1", "organization_id": rig.org, "url": "https://hooks.agentd.test/in", "secret": "whsec_c2VjcmV0"}]);
    let webhooks = Arc::new(Webhooks::parse(&json.to_string()).unwrap());
    rig.replica(&rig.stores, Streaming { webhooks, ..Streaming::default() })
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
    let (st, _) = call("DELETE", format!("/v1/vaults/{v}/credentials/{}", creds[0]), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "a repeated delete finds nothing and sends nothing");
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
    for c in &creds[1..] {
        assert_eq!(
            kinds(&sent(&rig, c).await),
            ["vault_credential.created", "vault_credential.archived", "vault_credential.deleted"]
        );
    }

    // B30: a vault deleted with no archive sends one deleted event per credential too.
    let (_, vault) = call("POST", "/v1/vaults".into(), Some(json!({"display_name": "w"}))).await;
    let w = vault["id"].as_str().unwrap().to_owned();
    let auth =
        json!({"type": "static_bearer", "mcp_server_url": "https://mcpw.agentd.test/mcp", "token": "t"});
    let (_, c) =
        call("POST", format!("/v1/vaults/{w}/credentials"), Some(json!({"display_name": "c", "auth": auth})))
            .await;
    let (st, _) = call("DELETE", format!("/v1/vaults/{w}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(kinds(&sent(&rig, &w).await), ["vault.created", "vault.deleted"]);
    let deleted = sent(&rig, c["id"].as_str().unwrap()).await;
    assert_eq!(kinds(&deleted), ["vault_credential.created", "vault_credential.deleted"]);
    assert_eq!(deleted[1]["data"]["vault_id"], w.as_str());
}
