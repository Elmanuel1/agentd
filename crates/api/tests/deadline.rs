//! The request deadline (SH-11, SH-12): a handler past it answers 504
//! `timeout_error` and its open transaction is rolled back; a stream is not
//! cut. A request whose checkout timed out is 529 (D14). The rollback case is
//! gated by `AGENTD_PG_TESTS=1`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::time::Duration;

use api::edge::deadline;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use diesel_async::{AsyncConnection, RunQueryDsl};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn get_once(app: Router, path: &str) -> (StatusCode, Vec<u8>) {
    let response = app.oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    (status, response.into_body().collect().await.unwrap().to_bytes().to_vec())
}

#[tokio::test]
async fn a_slow_handler_answers_504_timeout_error_and_a_stream_is_not_cut() {
    let slow = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            "late"
        }),
    );
    let (st, body) = get_once(deadline::bounded(slow, Duration::from_millis(100)), "/slow").await;
    assert_eq!(st, StatusCode::GATEWAY_TIMEOUT);
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["type"], "timeout_error");

    // The head comes at once; the body runs past the deadline.
    let stream = Router::new().route(
        "/stream",
        get(|| async {
            let chunks = futures::stream::unfold(0, |n| async move {
                (n < 3).then_some(())?;
                tokio::time::sleep(Duration::from_millis(100)).await;
                Some((Ok::<_, std::io::Error>(bytes::Bytes::from(format!("{n}"))), n + 1))
            });
            Body::from_stream(chunks)
        }),
    );
    let (st, body) = get_once(deadline::bounded(stream, Duration::from_millis(150)), "/stream").await;
    assert_eq!((st, body.as_slice()), (StatusCode::OK, b"012".as_slice()));
}

#[tokio::test]
async fn a_request_whose_checkout_timed_out_answers_529_overloaded_error() {
    // D14: whatever 5xx the route made of it.
    let waited = Router::new().route(
        "/busy",
        get(|| async {
            shared::overload::waited_out();
            StatusCode::INTERNAL_SERVER_ERROR
        }),
    );
    let (st, body) = get_once(waited.layer(axum::middleware::from_fn(deadline::shed)), "/busy").await;
    assert_eq!(st.as_u16(), 529);
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["type"], "overloaded_error");
}

#[tokio::test]
async fn a_request_that_waited_out_and_then_passed_its_deadline_stays_a_504() {
    let late = Router::new().route(
        "/late",
        get(|| async {
            shared::overload::waited_out();
            tokio::time::sleep(Duration::from_secs(5)).await;
            "late"
        }),
    );
    let app =
        deadline::bounded(late, Duration::from_millis(100)).layer(axum::middleware::from_fn(deadline::shed));
    let (st, body) = get_once(app, "/late").await;
    assert_eq!(st, StatusCode::GATEWAY_TIMEOUT);
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["type"], "timeout_error");
}

#[tokio::test]
async fn a_handler_past_the_deadline_has_its_transaction_rolled_back_and_its_locks_released() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let pg = store_postgres::PgBackend::connect(&std::env::var("DATABASE_URL").unwrap()).await.unwrap();
    let key = format!("ep_deadline_{}", std::process::id());
    let lock = 7_700_000_i64 + i64::from(std::process::id());
    let inserted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (held, sql, done) = (
        pg.clone(),
        format!(
            "INSERT INTO webhook_endpoint_states (endpoint_key, org_id, config_digest) VALUES ('{key}', 'o', 'd')"
        ),
        inserted.clone(),
    );
    let app = Router::new().route(
        "/write",
        get(move || {
            let (pg, sql, done) = (held.clone(), sql.clone(), done.clone());
            async move {
                let mut conn = pg.diesel.write().await.unwrap();
                conn.transaction::<_, diesel::result::Error, _>(async move |c| {
                    diesel::sql_query(format!("SELECT pg_advisory_xact_lock({lock})")).execute(c).await?;
                    diesel::sql_query(sql).execute(c).await?;
                    done.store(true, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok(())
                })
                .await
                .unwrap();
                "written"
            }
        }),
    );
    // Two idle connections, so the next checkout has a healthy one to take instead of the abandoned one.
    let warm = (pg.diesel.get().await.unwrap(), pg.diesel.get().await.unwrap());
    drop(warm);
    let (st, _) = get_once(deadline::bounded(app, Duration::from_millis(300)), "/write").await;
    assert_eq!(st, StatusCode::GATEWAY_TIMEOUT);
    assert!(inserted.load(std::sync::atomic::Ordering::SeqCst), "the insert ran before the deadline");
    #[derive(diesel::QueryableByName)]
    struct Free {
        #[diesel(sql_type = diesel::sql_types::Bool)]
        free: bool,
    }
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    // The next checkout takes the abandoned connection first and discards it, which ends its transaction.
    let mut conn = pg.diesel.get().await.unwrap();
    let probe = format!("SELECT pg_try_advisory_xact_lock({lock}) AS free");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !diesel::sql_query(&probe).get_result::<Free>(&mut conn).await.unwrap().free {
        assert!(tokio::time::Instant::now() < deadline, "the abandoned transaction still holds its lock");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let c: Count = diesel::sql_query(format!(
        "SELECT count(*) AS n FROM webhook_endpoint_states WHERE endpoint_key = '{key}'"
    ))
    .get_result(&mut conn)
    .await
    .unwrap();
    assert_eq!(c.n, 0, "the transaction was rolled back");
}
