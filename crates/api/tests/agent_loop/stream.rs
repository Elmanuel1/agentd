//! The loop's events on the session stream.

use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::common::{Auth, KEY, Rig, request};
use super::{Say, Scripted, message, on_loop, send, session};

/// The session's stream frames until one of type `until`, or 20 s.
async fn frames(app: &axum::Router, s: &str, until: &str) -> Vec<Value> {
    let req = request("GET", &format!("/v1/sessions/{s}/events/stream?beta=true"), Auth::Key(KEY), None);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body: Body = resp.into_body();
    let (mut buf, mut out) = (String::new(), Vec::new());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        while let Some(end) = buf.find("\n\n") {
            let frame: String = buf.drain(..end + 2).collect();
            if let Some(data) = frame.lines().find_map(|l| l.strip_prefix("data: ")) {
                let v: Value = serde_json::from_str(data).unwrap();
                let done = v["type"] == until;
                out.push(v);
                if done {
                    return out;
                }
            }
        }
        let Ok(Some(frame)) = tokio::time::timeout_at(deadline, body.frame()).await else { return out };
        if let Ok(bytes) = frame.unwrap().into_data() {
            buf.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

#[tokio::test]
async fn a_turn_s_usage_snapshot_reaches_the_session_stream_before_its_idle() {
    // Live's stream ends a turn with the thread's idle, `session.usage`, then the session's idle (probe 2026-11-10).
    let Some(rig) = Rig::new().await else { return };
    let model = Scripted::new(vec![Say::Text("hi there")]);
    let (app, _worker) = on_loop(&rig, model.clone()).await;
    let env = rig.environment("cloud").await;
    let s = session(&app, &env, json!([])).await;
    let app2 = app.clone();
    let s2 = s.clone();
    let reader = tokio::spawn(async move { frames(&app2, &s2, "session.status_idle").await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    send(&app, &s, message("hello")).await;
    let got = reader.await.unwrap();
    let types: Vec<&str> = got.iter().map(|e| e["type"].as_str().unwrap()).collect();
    let tail: Vec<&str> = types.iter().rev().take(3).rev().copied().collect();
    assert_eq!(tail, ["session.thread_status_idle", "session.usage", "session.status_idle"], "{types:?}");
    let usage = &got[got.len() - 2];
    assert_eq!(usage["usage"]["input_tokens"], 10);
    assert!(usage.get("budget").is_none(), "no budget, no `budget` key");
}
