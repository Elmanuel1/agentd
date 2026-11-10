//! The work API as the official worker's poller and worker use it, over live
//! Postgres: environment keys, the runtime key, work items, the work secret
//! and its `environment_variables`. Gated by `AGENTD_PG_TESTS=1` and
//! `AGENTD_AWS_ENDPOINT`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use common::{send_to, Auth, Rig, EGRESS_KEY, KEY, SCOPE, WORK_KEY};
use serde_json::{json, Value};
use shared::ports::secrets::SecretKey;
use work::{Placeholders, WorkTokens, RUNTIME};

fn secret(work: &Value) -> Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(work["secret"].as_str().unwrap()).unwrap()).unwrap()
}

#[tokio::test]
async fn a_self_hosted_worker_polls_acks_heartbeats_and_stops_its_environment_s_work() {
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let key = rig.environment_key(&env).await;
    let session = rig.session(&env, json!({})).await;
    rig.work.queue(&rig.ctx(), &env, &session, &env).await.unwrap();
    let base = format!("/v1/environments/{env}/work");

    let (st, stats) = rig.call("GET", &format!("{base}/stats"), Auth::Bearer(&key), None).await;
    assert_eq!(
        (st, stats["depth"].as_u64(), stats["type"].as_str()),
        (StatusCode::OK, Some(1), Some("work_queue_stats"))
    );
    // W-3, W-4, W-5, W-12: the session's id, actor null, started while queued, the secret once.
    let (st, work) = rig.call("GET", &format!("{base}/poll"), Auth::Bearer(&key), None).await;
    assert_eq!(st, StatusCode::OK, "{work}");
    assert_eq!(
        (work["id"].as_str(), work["data"]["id"].as_str()),
        (Some(session.as_str()), Some(session.as_str()))
    );
    assert_eq!((work["actor"].clone(), work["state"].as_str()), (Value::Null, Some("queued")));
    assert!(work["started_at"].is_string() && work.as_object().unwrap().contains_key("actor"));
    let decoded = secret(&work);
    assert_eq!(decoded["environment_variables"], json!({}), "EV-1: self_hosted gets none");
    let token = decoded["sessions_token"].as_str().unwrap().to_owned();
    assert_eq!(WorkTokens::new(WORK_KEY.to_vec()).verify(&token).unwrap().session, session);
    let (_, again) = rig.call("GET", &format!("{base}/poll"), Auth::Bearer(&key), None).await;
    assert_eq!(again, Value::Null, "handed out: no second poller gets it");
    let (_, stats) = rig.call("GET", &format!("{base}/stats"), Auth::Bearer(&key), None).await;
    assert_eq!((stats["depth"].as_u64(), stats["pending"].as_u64()), (Some(0), Some(1)));
    let (_, listed) = rig.call("GET", &base, Auth::Bearer(&key), None).await;
    assert_eq!(listed["data"][0]["id"].as_str(), Some(session.as_str()));
    assert_eq!(listed["data"][0]["secret"], Value::Null);

    let (st, acked) = rig.call("POST", &format!("{base}/{session}/ack"), Auth::Bearer(&key), None).await;
    assert_eq!((st, acked["state"].as_str()), (StatusCode::OK, Some("starting")));
    // W-7: no TTL, 300; W-21: 45 is granted and echoed; W-8: a string is live's 400.
    let beat = format!("{base}/{session}/heartbeat");
    let (st, first) = rig
        .call("POST", &format!("{beat}?expected_last_heartbeat=NO_HEARTBEAT"), Auth::Bearer(&token), None)
        .await;
    assert_eq!(st, StatusCode::OK, "{first}");
    assert_eq!(
        (first["ttl_seconds"].as_i64(), first["lease_extended"].as_bool(), first["state"].as_str()),
        (Some(300), Some(true), Some("active"))
    );
    let last = first["last_heartbeat"].as_str().unwrap().to_owned();
    let q = format!("{beat}?desired_ttl_seconds=45&expected_last_heartbeat={}", urlencode(&last));
    let (_, second) = rig.call("POST", &q, Auth::Bearer(&token), None).await;
    assert_eq!(second["ttl_seconds"].as_i64(), Some(45));
    let (_, zero) =
        rig.call("POST", &format!("{beat}?desired_ttl_seconds=0"), Auth::Bearer(&token), None).await;
    assert_eq!(zero["ttl_seconds"].as_i64(), Some(300));
    let (st, bad) =
        rig.call("POST", &format!("{beat}?desired_ttl_seconds=x"), Auth::Bearer(&token), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        bad["error"],
        json!({"type": "invalid_request_error", "message": "desired_ttl_seconds: Input should be a valid integer, unable to parse string as an integer"})
    );
    let (st, stale) = rig
        .call("POST", &format!("{beat}?expected_last_heartbeat=NO_HEARTBEAT"), Auth::Bearer(&token), None)
        .await;
    assert_eq!(st.as_u16(), 412, "{stale}");

    // W-4: another replica, or the same after a restart, serves the item.
    let req = Request::get(format!("{base}/{session}"))
        .header("anthropic-version", "2023-06-01")
        .header("x-api-key", KEY)
        .body(Body::empty())
        .unwrap();
    let (st, served) = send_to(&rig.restarted(), req).await;
    assert_eq!((st, served["state"].as_str()), (StatusCode::OK, Some("active")));

    let (st, stopped) =
        rig.call("POST", &format!("{base}/{session}/stop"), Auth::Bearer(&key), Some(json!({}))).await;
    assert_eq!((st, stopped["state"].as_str()), (StatusCode::OK, Some("stopping")), "a worker holds it");
    let (_, after) = rig.call("POST", &beat, Auth::Bearer(&token), None).await;
    assert_eq!((after["lease_extended"].as_bool(), after["state"].as_str()), (Some(false), Some("stopping")));
}

fn urlencode(s: &str) -> String {
    s.replace(':', "%3A").replace('+', "%2B")
}

#[tokio::test]
async fn an_environment_key_reaches_its_environment_s_sessions_only() {
    let Some(rig) = Rig::new().await else { return };
    let (env, other) = (rig.environment("self_hosted").await, rig.environment("self_hosted").await);
    let key = rig.environment_key(&env).await;
    let (mine, theirs) = (rig.session(&env, json!({})).await, rig.session(&other, json!({})).await);
    let (st, _) = rig.call("GET", &format!("/v1/sessions/{mine}"), Auth::Bearer(&key), None).await;
    assert_eq!(st, StatusCode::OK);
    // W-17.
    for path in [format!("/v1/sessions/{theirs}"), format!("/v1/sessions/{theirs}/events")] {
        let (st, body) = rig.call("GET", &path, Auth::Bearer(&key), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    let (st, _) = rig
        .call(
            "POST",
            &format!("/v1/sessions/{theirs}/events"),
            Auth::Bearer(&key),
            Some(json!({"events": []})),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // W-16: the environment itself, one work item, another queue.
    for path in [
        format!("/v1/environments/{env}"),
        format!("/v1/environments/{env}/work/{mine}"),
        format!("/v1/environments/{other}/work/poll"),
    ] {
        let (st, body) = rig.call("GET", &path, Auth::Bearer(&key), None).await;
        assert_eq!((st, body["error"]["message"].as_str()), (StatusCode::FORBIDDEN, Some(SCOPE)), "{path}");
        assert_eq!(body["error"]["type"], "permission_error");
    }
    // W-15, W-18.
    let (st, body) =
        rig.call("GET", &format!("/v1/environments/{env}/work/poll"), Auth::Key(&key), None).await;
    assert_eq!(
        (st, body["error"]["message"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid x-api-key"))
    );
    let (st, body) =
        rig.call("GET", &format!("/v1/environments/{env}/work/poll"), Auth::Key(KEY), None).await;
    assert_eq!(
        (st, body["error"]["message"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("Invalid bearer token"))
    );
    // W-13: the key is shown once; a listing shows its hint only.
    let listed = rig.env_keys.list().await.unwrap();
    let record = listed.iter().find(|k| k.environment_id == env).unwrap();
    assert!(record.partial_key_hint.contains("...") && record.partial_key_hint != key);
}

async fn vault(rig: &Rig, credentials: &[(&str, &str)]) -> (String, Vec<String>) {
    let (_, v) = rig.call("POST", "/v1/vaults", Auth::Key(KEY), Some(json!({"display_name": "v"}))).await;
    let id = v["id"].as_str().unwrap().to_owned();
    let mut ids = Vec::new();
    for (name, value) in credentials {
        let auth = json!({"type": "environment_variable", "secret_name": name, "secret_value": value, "networking": {"type": "unrestricted"}});
        let (st, c) = rig
            .call(
                "POST",
                &format!("/v1/vaults/{id}/credentials"),
                Auth::Key(KEY),
                Some(json!({"display_name": name, "auth": auth})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{c}");
        ids.push(c["id"].as_str().unwrap().to_owned());
    }
    (id, ids)
}

#[tokio::test]
async fn the_runtime_queue_hands_out_cloud_work_with_placeholders_and_never_a_value() {
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("cloud").await;
    let (first, first_creds) = vault(&rig, &[("SHARED", "value-one"), ("GONE", "value-gone")]).await;
    let (second, second_creds) = vault(&rig, &[("SHARED", "value-two"), ("OTHER", "value-three")]).await;
    let (st, _) = rig
        .call(
            "POST",
            &format!("/v1/vaults/{first}/credentials/{}/archive", first_creds[1]),
            Auth::Key(KEY),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    // EV-4: each environment_variable credential's secret is tagged when created; an MCP one is not.
    for (vault, credential) in [(&first, &first_creds[0]), (&second, &second_creds[1])] {
        assert!(rig.secrets.tagged(&SecretKey::new(&rig.ws, vault, credential).unwrap()));
    }
    let bearer =
        json!({"type": "static_bearer", "mcp_server_url": "https://mcp.agentd.test/sse", "token": "t"});
    let (st, mcp) = rig
        .call(
            "POST",
            &format!("/v1/vaults/{first}/credentials"),
            Auth::Key(KEY),
            Some(json!({"display_name": "m", "auth": bearer})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{mcp}");
    assert!(!rig.secrets.tagged(&SecretKey::new(&rig.ws, &first, mcp["id"].as_str().unwrap()).unwrap()));
    let session = rig.session(&env, json!({"vault_ids": [first, second]})).await;
    let runtime = rig.runtime().to_owned();
    // W-23 before any runtime work; then W-22 once the session's work is on the runtime queue.
    let (st, body) = rig.call("GET", &format!("/v1/environments/{env}"), Auth::Bearer(&runtime), None).await;
    assert_eq!((st, body["error"]["message"].as_str()), (StatusCode::FORBIDDEN, Some(SCOPE)));
    rig.work.queue(&rig.ctx(), &env, &session, RUNTIME).await.unwrap();
    let (st, read) = rig.call("GET", &format!("/v1/environments/{env}"), Auth::Bearer(&runtime), None).await;
    assert_eq!((st, read["id"].as_str()), (StatusCode::OK, Some(env.as_str())), "{read}");
    let (_, as_key) = rig.call("GET", &format!("/v1/environments/{env}"), Auth::Key(KEY), None).await;
    assert_eq!(read, as_key, "served as to a workspace key");

    let reads = rig.secrets.reads();
    // The runtime queue spans every workspace: other tests' items may come first.
    let mut work = Value::Null;
    for _ in 0..50 {
        let (st, polled) =
            rig.call("GET", "/v1/environments/runtime/work/poll", Auth::Bearer(&runtime), None).await;
        assert_eq!(st, StatusCode::OK, "{polled}");
        if polled["id"].as_str() == Some(session.as_str()) {
            work = polled;
            break;
        }
    }
    assert_eq!(work["id"].as_str(), Some(session.as_str()), "the session's runtime work is handed out");
    assert_eq!(rig.secrets.reads(), reads, "EV-3: a hand-out reads no secret");
    let decoded = secret(&work);
    let vars = decoded["environment_variables"].as_object().unwrap();
    assert_eq!(vars.keys().collect::<Vec<_>>(), ["OTHER", "SHARED"], "the archived credential is left out");
    let placeholders = Placeholders::new(EGRESS_KEY.to_vec());
    let shared = placeholders.verify(vars["SHARED"].as_str().unwrap()).unwrap();
    assert_eq!(
        (shared.vault.as_str(), shared.credential.as_str()),
        (first.as_str(), first_creds[0].as_str()),
        "the first vault wins"
    );
    let other = placeholders.verify(vars["OTHER"].as_str().unwrap()).unwrap();
    assert_eq!(
        (other.session.as_str(), other.ws.as_str(), other.credential.as_str()),
        (session.as_str(), rig.ws.as_str(), second_creds[1].as_str())
    );
    // EV-3: no value in the secret or the work object.
    let raw = format!("{work}{decoded}");
    for value in ["value-one", "value-two", "value-three", "value-gone"] {
        assert!(!raw.contains(value), "{value}");
    }
    // The runtime key acks and stops the item it polled; the worker heartbeats with the token.
    let (st, acked) = rig
        .call("POST", &format!("/v1/environments/runtime/work/{session}/ack"), Auth::Bearer(&runtime), None)
        .await;
    assert_eq!((st, acked["state"].as_str()), (StatusCode::OK, Some("starting")), "{acked}");
    let token = decoded["sessions_token"].as_str().unwrap();
    let (st, beat) = rig
        .call(
            "POST",
            &format!("/v1/environments/runtime/work/{session}/heartbeat"),
            Auth::Bearer(token),
            None,
        )
        .await;
    assert_eq!((st, beat["state"].as_str()), (StatusCode::OK, Some("active")), "{beat}");
    let (st, s) = rig.call("GET", &format!("/v1/sessions/{session}"), Auth::Bearer(&runtime), None).await;
    assert_eq!((st, s["id"].as_str()), (StatusCode::OK, Some(session.as_str())));
    let (st, stopped) = rig
        .call(
            "POST",
            &format!("/v1/environments/runtime/work/{session}/stop"),
            Auth::Bearer(&runtime),
            Some(json!({"force": true})),
        )
        .await;
    assert_eq!((st, stopped["state"].as_str()), (StatusCode::OK, Some("stopped")));
    // L-12: the token is refused once its item is stopped.
    let (st, _) = rig.call("GET", &format!("/v1/sessions/{session}"), Auth::Bearer(token), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // The runtime key never reaches another queue, nor a session off the runtime queue.
    let off = rig.session(&env, json!({})).await;
    let (st, _) = rig.call("GET", &format!("/v1/sessions/{off}"), Auth::Bearer(&runtime), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_archived_or_deleted_session_s_item_stops_and_is_kept() {
    // W-11, on the harness path: each session's sandbox takes its own hosted item.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let env = rig.environment("cloud").await;
    let (idle, held) = (rig.session(&env, json!({})).await, rig.session(&env, json!({})).await);
    let token = rig.token(&held);
    let path = |s: &str| format!("/v1/environments/{env}/work/{s}");
    let (st, acked) = rig.call("POST", &format!("{}/ack", path(&held)), Auth::Bearer(&token), None).await;
    assert_eq!((st, acked["state"].as_str()), (StatusCode::OK, Some("starting")), "{acked}");
    for s in [&idle, &held] {
        let (st, _) = rig.call("POST", &format!("/v1/sessions/{s}/archive"), Auth::Key(KEY), None).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (_, queued) = rig.call("GET", &path(&idle), Auth::Key(KEY), None).await;
    assert_eq!(queued["state"], "stopped", "no worker held it");
    let (_, stopping) = rig.call("GET", &path(&held), Auth::Key(KEY), None).await;
    assert_eq!(stopping["state"], "stopping", "its worker finishes");
    let (st, _) = rig.call("DELETE", &format!("/v1/sessions/{idle}"), Auth::Key(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, kept) = rig.call("GET", &path(&idle), Auth::Key(KEY), None).await;
    assert_eq!(
        (st, kept["state"].as_str()),
        (StatusCode::OK, Some("stopped")),
        "the item outlives its session"
    );
}
