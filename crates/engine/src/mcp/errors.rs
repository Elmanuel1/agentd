//! The `session.error` bodies and tool results a failed MCP server gets, in
//! live's words where a capture shows them (`.specs/runtime-parity`, B27).

use serde_json::{json, Value};
use sessions::events::{Event, EventPayload, SessionEvent};
use shared::ContentBlock;

use super::client::Failure;

/// The `session.error` type of a server agentd could not reach.
pub const CONNECTION_FAILED: &str = "mcp_connection_failed_error";
/// The `session.error` type of a server that refused agentd's credential.
const AUTHENTICATION_FAILED: &str = "mcp_authentication_failed_error";

/// Why `stage` (`initialize`, `tools/call`) failed, in live's words.
fn reason(failure: &Failure) -> String {
    match failure {
        Failure::Unreachable => {
            "the server host could not be resolved or reached \u{2014} check the server URL".into()
        }
        Failure::Forbidden => "access forbidden".into(),
        Failure::Unauthorized => "authentication required".into(),
        Failure::Unauthenticated => {
            "no credential is stored for this server URL \u{2014} check that the agent's MCP \
            server URL matches the URL in the vault"
                .into()
        }
        Failure::Gone => "the server ended the session".into(),
        Failure::Refresh => "the access token expired and could not be refreshed".into(),
        Failure::Busy => "the access token is being refreshed".into(),
        Failure::Status(code) => format!("unexpected HTTP status {code}"),
    }
}

/// The `session.error` body for `server` failing at `stage`; the next turn
/// tries the server again, so it is always `retrying` (MC-14).
pub(super) fn session_error(server: &str, stage: &str, failure: &Failure) -> Value {
    let kind = match failure {
        Failure::Unauthorized | Failure::Unauthenticated | Failure::Refresh => AUTHENTICATION_FAILED,
        _ => CONNECTION_FAILED,
    };
    json!({
        "type": kind,
        "mcp_server_name": server,
        "message": format!("MCP server '{server}' {stage} failed: {}", reason(failure)),
        "retry_status": {"type": "retrying"},
    })
}

/// The result of a call the server did not answer, in live's words (B27).
pub(super) fn unanswered(failure: &Failure) -> Vec<ContentBlock> {
    let why = match failure {
        Failure::Status(code) => format!("upstream server error (HTTP {code})"),
        Failure::Unauthorized | Failure::Unauthenticated => "upstream server error (HTTP 401)".into(),
        Failure::Forbidden => "upstream server error (HTTP 403)".into(),
        Failure::Gone => "upstream server error (HTTP 404)".into(),
        _ => "upstream server error (unreachable)".into(),
    };
    vec![ContentBlock::text(why)]
}

/// Whether `e` is an MCP server's `session.error`, and for which server.
pub fn is_mcp_error(e: &Event) -> Option<&str> {
    let EventPayload::Session(SessionEvent::Error { error }) = &e.payload else { return None };
    let kind = error.get("type").and_then(Value::as_str)?;
    [CONNECTION_FAILED, AUTHENTICATION_FAILED]
        .contains(&kind)
        .then(|| error.get("mcp_server_name").and_then(Value::as_str))
        .flatten()
}
