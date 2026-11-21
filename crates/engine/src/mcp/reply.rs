//! An MCP server's reply: read chunk by chunk up to 16 MiB, so one server
//! cannot fill the replica's memory (SH-26); a longer reply fails the call as
//! an unreachable server does. Its messages are one JSON value, an array of
//! them, or an event stream's `data` fields.

use reqwest::header::{CONTENT_TYPE, HeaderMap};
use serde_json::Value;

use super::client::Failure;

/// The longest reply read: 160 times live's 100,000-character threshold for an output file (L8).
pub(super) const MOST: usize = 16 * 1024 * 1024;

/// The reply's body, unless it runs past [`MOST`].
pub(super) async fn read(mut reply: reqwest::Response) -> Result<String, Failure> {
    let mut body = Vec::new();
    while let Some(chunk) = reply.chunk().await.map_err(|_| Failure::Unreachable)? {
        if body.len() + chunk.len() > MOST {
            return Err(Failure::Unreachable);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// The JSON-RPC messages in `text`, by the reply's content type.
pub(super) fn messages(headers: &HeaderMap, text: &str) -> Vec<Value> {
    let streamed = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("text/event-stream"));
    if streamed {
        return events(text);
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(all)) => all,
        Ok(one) => vec![one],
        Err(_) => Vec::new(),
    }
}

/// The JSON messages of an event stream's `data` fields.
fn events(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .flat_map(|chunk| chunk.split("\r\n\r\n"))
        .filter_map(|event| {
            let data: Vec<&str> = event
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            (!data.is_empty()).then(|| serde_json::from_str(&data.join("\n")).ok()).flatten()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use axum::routing::post;

    use super::*;

    #[test]
    fn an_event_stream_yields_each_data_message() {
        let text = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n\
                    id: 2\ndata: {\"jsonrpc\":\"2.0\",\n data: \"id\":1,\"result\":{}}\n\n";
        let got = events(text);
        assert_eq!(got.len(), 1, "a data line that does not start the field is not one");
        assert_eq!(got[0]["method"], "notifications/tools/list_changed");
        let split = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":1,\"result\":{}}\n\n";
        assert_eq!(events(split)[0]["id"], 1);
    }

    #[tokio::test]
    async fn a_reply_over_16_mib_fails_the_call() {
        let app = axum::Router::new()
            .route("/big", post(|| async { "x".repeat(MOST + 1) }))
            .route("/fits", post(|| async { "x".repeat(MOST) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        let client = reqwest::Client::new();
        let big = client.post(format!("http://{addr}/big")).send().await.unwrap();
        assert!(matches!(read(big).await, Err(Failure::Unreachable)));
        let fits = client.post(format!("http://{addr}/fits")).send().await.unwrap();
        assert_eq!(read(fits).await.ok().map(|b| b.len()), Some(MOST));
    }
}
