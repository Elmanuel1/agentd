//! Session streams across replicas: two apps over one database, signalling
//! through Valkey (`AGENTD_VALKEY_URL`) or not at all; the overload gate;
//! the read replica; and the session delete. Gated by `AGENTD_PG_TESTS=1`
//! and `AGENTD_AWS_ENDPOINT`; the Valkey tests also need `AGENTD_VALKEY_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use axum::Router;
use common::{request, send_to, Auth, Rig, Streaming, KEY};
use diesel::connection::{Instrumentation, InstrumentationEvent};
use http_body_util::BodyExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use sessions::store::DatabaseLoad;
use sessions::SessionId;
use shared::WorkspaceId;
use signals_valkey::ValkeySignals;
use store_postgres::{PgBackend, PgObserver, PoolSettings};
use tower::ServiceExt;

/// An open SSE response, read frame by frame.
struct Sse {
    body: Body,
    buffered: String,
}

impl Sse {
    async fn open(app: &Router, session: &str) -> Self {
        let req = request("GET", &format!("/v1/sessions/{session}/events/stream"), Auth::Key(KEY), None);
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        Self { body: resp.into_body(), buffered: String::new() }
    }

    /// The next event within `wait`: `Some(None)` once the stream ends,
    /// `None` when nothing came.
    async fn next(&mut self, wait: Duration) -> Option<Option<Value>> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(end) = self.buffered.find("\n\n") {
                let frame: String = self.buffered.drain(..end + 2).collect();
                let data = frame.lines().find_map(|l| l.strip_prefix("data: "));
                if let Some(data) = data {
                    return Some(Some(serde_json::from_str(data).unwrap()));
                }
                continue;
            }
            let frame = tokio::time::timeout_at(deadline, self.body.frame()).await.ok()?;
            let Some(frame) = frame else { return Some(None) };
            if let Ok(bytes) = frame.unwrap().into_data() {
                self.buffered.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }

    async fn event(&mut self, wait: Duration) -> Value {
        self.next(wait).await.expect("an event in time").expect("the stream still open")
    }
}

async fn valkey() -> Option<Streaming> {
    let url = std::env::var("AGENTD_VALKEY_URL").ok()?;
    Some(Streaming { signals: Arc::new(ValkeySignals::connect(&url).unwrap()), ..Streaming::default() })
}

fn message(text: &str) -> Value {
    json!({"events": [{"type": "user.message", "content": [{"type": "text", "text": text}]}]})
}

/// The session's harness takes its queued client events, which stamps them processed.
async fn take(app: &Router, rig: &Rig, session: &str) {
    let take = format!("/v1/sessions/{session}/harness/take");
    let (st, taken) = send_to(app, request("POST", &take, Auth::Bearer(&rig.token(session)), None)).await;
    assert_eq!(st, StatusCode::OK, "{taken}");
}

/// `event` as it was sent: processed, with its `processed_at` taken off.
fn as_sent(mut event: Value) -> Value {
    assert!(event["processed_at"].is_string(), "{event}");
    event.as_object_mut().unwrap().remove("processed_at");
    event
}

/// Within the signal path's reach, well inside the 5 s fallback timer.
const SIGNALLED: Duration = Duration::from_secs(3);

#[tokio::test]
async fn an_event_sent_through_one_replica_reaches_another_s_stream_in_order_once_processed() {
    // F-1, over Valkey.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let (Some(a), Some(b)) = (valkey().await, valkey().await) else { return };
    let (a, b) = (rig.replica(&rig.stores, a), rig.replica(&rig.stores, b));
    let env = rig.environment("cloud").await;
    let session = rig.session(&env, json!({})).await;
    let mut stream = Sse::open(&b, &session).await;
    let events = format!("/v1/sessions/{session}/events");
    let (st, first) = send_to(&a, request("POST", &events, Auth::Key(KEY), Some(message("one")))).await;
    assert_eq!(st, StatusCode::OK, "{first}");
    let (_, second) = send_to(&a, request("POST", &events, Auth::Key(KEY), Some(message("two")))).await;
    let sent = [first["data"][0].clone(), second["data"][0].clone()];
    assert_eq!(stream.next(SIGNALLED).await, None, "live streams no queued copy");
    take(&a, &rig, &session).await;
    for queued in &sent {
        assert_eq!(&as_sent(stream.event(SIGNALLED).await), queued, "the same event, processed, in order");
    }
}

#[tokio::test]
async fn a_stream_opened_while_an_event_is_queued_gets_it_once_processed() {
    // F-1: the stamp of an event sent before the stream opened still reaches it.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let (Some(a), Some(b)) = (valkey().await, valkey().await) else { return };
    let (a, b) = (rig.replica(&rig.stores, a), rig.replica(&rig.stores, b));
    let env = rig.environment("cloud").await;
    let session = rig.session(&env, json!({})).await;
    let events = format!("/v1/sessions/{session}/events");
    let (_, sent) = send_to(&a, request("POST", &events, Auth::Key(KEY), Some(message("early")))).await;
    let mut stream = Sse::open(&b, &session).await;
    take(&a, &rig, &session).await;
    assert_eq!(as_sent(stream.event(SIGNALLED).await), sent["data"][0]);
}

#[tokio::test]
async fn deleting_through_one_replica_ends_another_s_stream_with_session_deleted() {
    // F-2, over Valkey.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let (Some(a), Some(b)) = (valkey().await, valkey().await) else { return };
    let (a, b) = (rig.replica(&rig.stores, a), rig.replica(&rig.stores, b));
    let env = rig.environment("cloud").await;
    let session = rig.session(&env, json!({})).await;
    let mut stream = Sse::open(&b, &session).await;
    let (st, _) =
        send_to(&a, request("DELETE", &format!("/v1/sessions/{session}"), Auth::Key(KEY), None)).await;
    assert_eq!(st, StatusCode::OK);
    let last = stream.event(SIGNALLED).await;
    assert_eq!(
        (last["type"].as_str(), last["processed_at"].is_string()),
        (Some("session.deleted"), true),
        "{last}"
    );
    assert_eq!(stream.next(SIGNALLED).await, Some(None), "the stream ends after it");
}

#[tokio::test]
async fn archiving_through_one_replica_ends_another_s_stream() {
    // F-3, over Valkey.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let (Some(a), Some(b)) = (valkey().await, valkey().await) else { return };
    let (a, b) = (rig.replica(&rig.stores, a), rig.replica(&rig.stores, b));
    let env = rig.environment("cloud").await;
    let session = rig.session(&env, json!({})).await;
    let mut stream = Sse::open(&b, &session).await;
    let archive = format!("/v1/sessions/{session}/archive");
    let (st, _) = send_to(&a, request("POST", &archive, Auth::Key(KEY), None)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(stream.next(SIGNALLED).await, Some(None));
}

#[tokio::test]
async fn with_no_signal_between_replicas_an_event_still_arrives_within_six_seconds() {
    // F-4: each replica signals only itself, so the fallback timer delivers.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let (a, b) =
        (rig.replica(&rig.stores, Streaming::default()), rig.replica(&rig.stores, Streaming::default()));
    let env = rig.environment("cloud").await;
    let session = rig.session(&env, json!({})).await;
    let mut stream = Sse::open(&b, &session).await;
    let events = format!("/v1/sessions/{session}/events");
    let (_, sent) = send_to(&a, request("POST", &events, Auth::Key(KEY), Some(message("late")))).await;
    take(&a, &rig, &session).await;
    let started = tokio::time::Instant::now();
    assert_eq!(as_sent(stream.event(Duration::from_millis(6500)).await), sent["data"][0]);
    assert!(started.elapsed() > Duration::from_millis(100), "no signal reached the other replica");
}

struct Switch(AtomicBool);

impl DatabaseLoad for Switch {
    fn overloaded(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[tokio::test]
async fn while_overloaded_a_new_stream_gets_529_and_an_open_one_keeps_streaming() {
    // F-5, with the gate forced.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let gate = Arc::new(Switch(AtomicBool::new(false)));
    let app = rig.replica(&rig.stores, Streaming { load: gate.clone(), ..Streaming::default() });
    let env = rig.environment("cloud").await;
    let session = rig.session(&env, json!({})).await;
    let mut open = Sse::open(&app, &session).await;
    gate.0.store(true, Ordering::Relaxed);
    let stream = format!("/v1/sessions/{session}/events/stream");
    let (st, refused) = send_to(&app, request("GET", &stream, Auth::Key(KEY), None)).await;
    assert_eq!(st.as_u16(), 529);
    assert_eq!(refused["error"], json!({"type": "overloaded_error", "message": "Overloaded"}), "{refused}");
    let events = format!("/v1/sessions/{session}/events");
    let (_, sent) = send_to(&app, request("POST", &events, Auth::Key(KEY), Some(message("still")))).await;
    take(&app, &rig, &session).await;
    assert_eq!(as_sent(open.event(SIGNALLED).await), sent["data"][0]);
}

/// The statements a pool runs, in order.
#[derive(Default)]
struct Seen(Arc<Mutex<Vec<String>>>);

impl PgObserver for Seen {
    fn statements(&self) -> Box<dyn Instrumentation> {
        Box::new(Recorder(self.0.clone()))
    }

    fn connect_failed(&self, _: &diesel::ConnectionError) {}
}

struct Recorder(Arc<Mutex<Vec<String>>>);

impl Instrumentation for Recorder {
    fn on_connection_event(&mut self, event: InstrumentationEvent<'_>) {
        if let InstrumentationEvent::StartQuery { query, .. } = event {
            self.0.lock().push(query.to_string());
        }
    }
}

#[tokio::test]
async fn with_a_read_replica_only_the_events_list_reads_it() {
    // E-3: the list on the replica; sends, stamps and streams on the primary.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let seen = Seen::default();
    let statements = seen.0.clone();
    let replica = PgBackend::connect_observed(&url, PoolSettings::default(), Arc::new(seen)).await.unwrap();
    let stores = rig.stores.clone().reading_events_from(replica);
    let app = rig.replica(&stores, Streaming::default());
    let env = rig.environment("cloud").await;
    let session = rig.session(&env, json!({})).await;
    let mut stream = Sse::open(&app, &session).await;
    let events = format!("/v1/sessions/{session}/events");
    let (_, sent) = send_to(&app, request("POST", &events, Auth::Key(KEY), Some(message("primary")))).await;
    take(&app, &rig, &session).await;
    assert_eq!(as_sent(stream.event(SIGNALLED).await), sent["data"][0]);
    assert_eq!(statements.lock().clone(), Vec::<String>::new(), "no send, stamp or stream read the replica");

    let (st, listed) = send_to(&app, request("GET", &events, Auth::Key(KEY), None)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);
    let read = statements.lock().clone();
    assert_eq!(read.len(), 2, "{read:?}");
    assert!(read[0].contains("FROM \"sessions\""), "the session check: {}", read[0]);
    assert!(read[1].contains("FROM session_events"), "the page: {}", read[1]);
}

#[tokio::test]
async fn a_message_racing_a_delete_lands_before_it_or_is_refused_and_leaves_no_event() {
    // E-4, looped: the message is accepted and the delete then removes it, or
    // the message finds no session; either way no event outlives the session
    // and its work item reads stopped.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let env = rig.environment("cloud").await;
    let ws = WorkspaceId::new(&rig.ws);
    let mut outcomes = std::collections::BTreeSet::new();
    for _ in 0..10 {
        let session = rig.session(&env, json!({})).await;
        let events = format!("/v1/sessions/{session}/events");
        let delete = format!("/v1/sessions/{session}");
        let ((sent, _), (deleted, _)) = tokio::join!(
            rig.call("POST", &events, Auth::Key(KEY), Some(message("race"))),
            rig.call("DELETE", &delete, Auth::Key(KEY), None),
        );
        assert_eq!(deleted, StatusCode::OK);
        assert!(matches!(sent, StatusCode::OK | StatusCode::NOT_FOUND), "{sent}");
        outcomes.insert(sent.as_u16());
        let id = SessionId::parse(&session).unwrap();
        assert_eq!(
            rig.stores.session_events.head(&ws, &id).await.unwrap().seq,
            0,
            "no event outlives its session"
        );
        let (_, item) =
            rig.call("GET", &format!("/v1/environments/{env}/work/{session}"), Auth::Key(KEY), None).await;
        assert_eq!(item["state"], "stopped");
    }
    println!("outcomes seen: {outcomes:?}");
}

#[tokio::test]
async fn a_running_session_is_not_deleted() {
    // E-4: the delete answers 409 and keeps the session and its events.
    let Some(rig) = Rig::with_sandbox().await else { return };
    let env = rig.environment("cloud").await;
    let session = rig.session(&env, json!({})).await;
    let harness = format!("/v1/sessions/{session}/harness/events");
    let running = json!({"events": [{"type": "session.status_running"}]});
    let (st, body) = rig.call("POST", &harness, Auth::Bearer(&rig.token(&session)), Some(running)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, refused) = rig.call("DELETE", &format!("/v1/sessions/{session}"), Auth::Key(KEY), None).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(refused["error"]["message"], "cannot delete a running session");
    let (st, _) = rig.call("GET", &format!("/v1/sessions/{session}"), Auth::Key(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
}
