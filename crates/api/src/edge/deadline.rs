//! Each module's request deadline: a response whose head has not started
//! within it is 504 `timeout_error` (L3). The handler's future is dropped, so
//! a transaction it held is rolled back: its connection is marked broken and
//! the pool discards it (DEP-13), and `statement_timeout` ends a statement
//! still running on the server. A stream returns its head at once, so its
//! body is never cut. A request whose database checkout timed out answers
//! 529 `overloaded_error` (D14).

use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::Response;
use shared::{ApiError, ErrorType, wire_response};

/// `routes` answered 504 when their response has not started within `deadline`.
pub fn bounded(routes: Router, deadline: Duration) -> Router {
    routes.layer(from_fn_with_state(deadline, within))
}

async fn within(State(deadline): State<Duration>, req: Request, next: Next) -> Response {
    match tokio::time::timeout(deadline, shared::overload::watched(next.run(req))).await {
        Ok((response, false)) => response,
        Ok((response, true)) if !response.status().is_server_error() => response,
        Ok((_, true)) => {
            crate::telemetry::edge::rejected("overloaded");
            wire_response(&ApiError::of(ErrorType::Overloaded, "no database connection came free in time"))
        }
        Err(_) => {
            crate::telemetry::edge::rejected("timeout");
            tracing::info!(
                ms = deadline.as_millis(),
                "edge: a request passed its deadline before its response started"
            );
            wire_response(&ApiError::timeout(format!(
                "the response did not start within {} ms",
                deadline.as_millis()
            )))
        }
    }
}
