//! Keeps the models agents may name current from each provider's model
//! list: the Messages API's (`GET /v1/models`) for Anthropic's ids, and the
//! OpenAI-compatible API's (`GET /models`, OpenRouter's) for `vendor/model`
//! ids, each read at start and once a day. A read that fails keeps the list
//! last read: before any, Anthropic's built-in list and no `vendor/model` id.

use std::time::Duration;

use serde_json::Value;
use shared::limits::ModelCatalog;

use super::ModelError;
use super::anthropic::AnthropicModel;
use super::openai::OpenAiCompatModel;
use super::windows::get as fetch;

/// How often a list is read again.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long one page's read may take.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Read the Messages API's list at `base_url` with `api_key` now and once a
/// day, for as long as the process runs, into `catalog`.
pub fn spawn_anthropic(
    api_key: String,
    base_url: String,
    catalog: ModelCatalog,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(http) = client("anthropic") else { return };
        let api = AnthropicModel::new(http, api_key, base_url);
        loop {
            install("anthropic", api.list_models().await, |ids| catalog.replace_anthropic(ids));
            tokio::time::sleep(EVERY).await;
        }
    })
}

/// Read the OpenAI-compatible API's list at `base_url` with `api_key` now
/// and once a day, for as long as the process runs, into `catalog`.
pub fn spawn_openai_compat(
    api_key: String,
    base_url: String,
    catalog: ModelCatalog,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Some(http) = client("openai_compat") else { return };
        let api = OpenAiCompatModel::new(http, api_key, base_url);
        loop {
            let read = fetch(api.get("/models")).await.map(|list| ids(&list));
            install("openai_compat", read, |ids| catalog.replace_openai_compat(ids));
            tokio::time::sleep(EVERY).await;
        }
    })
}

fn client(provider: &str) -> Option<reqwest::Client> {
    let built = reqwest::Client::builder().timeout(TIMEOUT).build();
    built.inspect_err(|e| tracing::warn!(provider, error = %e, "models: no HTTP client")).ok()
}

fn install(provider: &str, read: Result<Vec<String>, ModelError>, replace: impl FnOnce(Vec<String>)) {
    match read {
        Ok(ids) if !ids.is_empty() => {
            tracing::debug!(provider, models = ids.len(), "models: the list read");
            replace(ids);
        }
        Ok(_) => tracing::warn!(provider, "models: the list is empty; the models stay"),
        Err(e) => tracing::warn!(provider, error = %e, "models: the list cannot be read; the models stay"),
    }
}

/// The ids of one page of a model list.
fn ids(page: &Value) -> Vec<String> {
    let listed = page.get("data").and_then(Value::as_array).into_iter().flatten();
    listed.filter_map(|m| m.get("id").and_then(Value::as_str)).map(str::to_owned).collect()
}

impl AnthropicModel {
    /// Every id the Messages API lists, page by page.
    async fn list_models(&self) -> Result<Vec<String>, ModelError> {
        let mut all = Vec::new();
        let mut path = "/v1/models?limit=1000".to_owned();
        loop {
            let page = fetch(self.get(&path)).await?;
            all.extend(ids(&page));
            let more = page.get("has_more").and_then(Value::as_bool) == Some(true);
            match page.get("last_id").and_then(Value::as_str).filter(|_| more) {
                Some(last) => path = format!("/v1/models?limit=1000&after_id={last}"),
                None => return Ok(all),
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
        drop(spawn_anthropic("k".into(), serve(app).await, catalog.clone()));
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
        drop(spawn_anthropic("k".into(), serve(app).await, catalog.clone()));
        until(|| hits.load(Ordering::SeqCst) > 0).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(catalog.supports("claude-haiku-5-5"));
    }

    fn listed() -> serde_json::Value {
        json!({"data": [{"id": "qwen/qwen3.7-flash:free", "context_length": 262144},
            {"id": "openai/gpt-6-luna", "context_length": 400000}]})
    }

    #[tokio::test]
    async fn the_openai_compatible_list_is_the_vendor_models_accepted() {
        let app = Router::new().route("/api/v1/models", get(|| async { Json(listed()) }));
        let catalog = ModelCatalog::default_catalog();
        assert!(!catalog.supports("openai/gpt-6-luna"), "none before the first read");
        drop(spawn_openai_compat("k".into(), format!("{}/api/v1", serve(app).await), catalog.clone()));
        until(|| catalog.supports("openai/gpt-6-luna")).await;
        assert!(catalog.supports("qwen/qwen3.7-flash:free"));
        assert!(!catalog.supports("qwen/qwen3.7-flash"), "a variant only as listed");
        assert!(catalog.supports("claude-haiku-5-5"), "Anthropic's list stays");
    }
}
