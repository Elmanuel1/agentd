//! Keeps the Anthropic models agents may name current from the Messages
//! API's model list (`GET /v1/models`): read at start and once a day. A read
//! that fails keeps the models last read, or the built-in list before any.

use std::time::Duration;

use serde_json::Value;
use shared::limits::ModelCatalog;

use super::ModelError;
use super::anthropic::AnthropicModel;
use super::windows::get as fetch;

/// How often the list is read again.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long one page's read may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Read the list at `base_url` with `api_key` now and once a day, for as
/// long as the process runs, into `catalog`.
pub fn spawn(api_key: String, base_url: String, catalog: ModelCatalog) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let http = match reqwest::Client::builder().timeout(TIMEOUT).build() {
            Ok(http) => http,
            Err(e) => {
                tracing::warn!(error = %e, "models: no HTTP client; the built-in Anthropic models stay");
                return;
            }
        };
        let api = AnthropicModel::new(http, api_key, base_url);
        loop {
            match api.list_models().await {
                Ok(ids) if !ids.is_empty() => {
                    tracing::debug!(models = ids.len(), "models: Anthropic's model list read");
                    catalog.replace_anthropic(ids);
                }
                Ok(_) => tracing::warn!("models: Anthropic's model list is empty; the models stay"),
                Err(e) => {
                    tracing::warn!(error = %e, "models: Anthropic's model list cannot be read; the models stay")
                }
            }
            tokio::time::sleep(EVERY).await;
        }
    })
}

impl AnthropicModel {
    /// Every id the Messages API lists, page by page.
    async fn list_models(&self) -> Result<Vec<String>, ModelError> {
        let mut ids = Vec::new();
        let mut path = "/v1/models?limit=1000".to_owned();
        loop {
            let page = fetch(self.get(&path)).await?;
            let listed = page.get("data").and_then(Value::as_array).into_iter().flatten();
            ids.extend(listed.filter_map(|m| m.get("id").and_then(Value::as_str)).map(str::to_owned));
            let more = page.get("has_more").and_then(Value::as_bool) == Some(true);
            match page.get("last_id").and_then(Value::as_str).filter(|_| more) {
                Some(last) => path = format!("/v1/models?limit=1000&after_id={last}"),
                None => return Ok(ids),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::RawQuery;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;

    use super::*;

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        base
    }

    async fn until(done: impl Fn() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn every_listed_page_replaces_the_built_in_models() {
        let app = Router::new().route(
            "/v1/models",
            get(|RawQuery(q): RawQuery| async move {
                match q.unwrap_or_default().contains("after_id=claude-new-1") {
                    false => Json(json!({"data": [{"id": "claude-new-1"}], "has_more": true,
                        "first_id": "claude-new-1", "last_id": "claude-new-1"})),
                    true => Json(json!({"data": [{"id": "claude-old-1-20250101"}], "has_more": false,
                        "first_id": "claude-old-1-20250101", "last_id": "claude-old-1-20250101"})),
                }
            }),
        );
        let catalog = ModelCatalog::default_catalog();
        drop(spawn("k".into(), serve(app).await, catalog.clone()));
        until(|| catalog.supports("claude-old-1")).await;
        for id in ["claude-new-1", "claude-old-1-20250101", "claude-old-1"] {
            assert!(catalog.supports(id), "{id}");
        }
        assert!(!catalog.supports("claude-haiku-5-5"), "a model the list lacks");
    }

    #[tokio::test]
    async fn a_failed_read_keeps_the_built_in_models() {
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        let app = Router::new().route(
            "/v1/models",
            get(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::INTERNAL_SERVER_ERROR }
            }),
        );
        let catalog = ModelCatalog::default_catalog();
        drop(spawn("k".into(), serve(app).await, catalog.clone()));
        until(|| hits.load(Ordering::SeqCst) > 0).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(catalog.supports("claude-haiku-5-5"));
    }
}
