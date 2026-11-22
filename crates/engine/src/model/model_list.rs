//! Keeps the models agents may name current from each provider's model
//! list, read on the [`super::daily`] schedule: the Messages API's
//! (`GET /v1/models`) for Anthropic's ids, and the OpenAI-compatible API's
//! (`GET /models`, OpenRouter's) for `vendor/model` ids. Until a read
//! succeeds, Anthropic's built-in list and no `vendor/model` id are accepted.

use serde_json::Value;
use shared::limits::ModelCatalog;

use super::ProviderConfig;
use super::anthropic::AnthropicModel;
use super::daily;
use super::openai::OpenAiCompatModel;

/// Start the read of each model list `providers` sets a key for, into
/// `catalog`, for as long as the process runs.
pub fn spawn(providers: &ProviderConfig, catalog: &ModelCatalog) {
    if let Some((key, url)) = providers.anthropic.clone() {
        let read = move |http| {
            let api = AnthropicModel::new(http, key.clone(), url.clone());
            async move {
                let (mut all, mut seen) = (Vec::new(), Vec::new());
                let mut path = "/v1/models?limit=1000".to_owned();
                loop {
                    let page = daily::json(api.get(&path)).await?;
                    all.extend(ids(&page));
                    if page.get("has_more").and_then(Value::as_bool) != Some(true) {
                        return Ok::<_, String>(all);
                    }
                    let next = page.get("last_id").and_then(Value::as_str).map(str::to_owned);
                    let Some(last) = next.filter(|l| !seen.contains(l)) else {
                        return Err("the model list's next page has no new cursor".to_owned());
                    };
                    path = format!("/v1/models?limit=1000&after_id={last}");
                    seen.push(last);
                }
            }
        };
        let models = catalog.clone();
        drop(daily::spawn("anthropic models", read, move |found| models.replace_anthropic(found)));
    }
    if let Some((key, url)) = providers.openai_compat.clone() {
        let read = move |http| {
            let req = OpenAiCompatModel::new(http, key.clone(), url.clone()).get("/models");
            async move {
                let list = daily::json(req).await?;
                Ok::<_, String>(ids(&list).into_iter().filter(|id| id.contains('/')).collect::<Vec<_>>())
            }
        };
        let models = catalog.clone();
        drop(daily::spawn("openai_compat models", read, move |found| models.replace_openai_compat(found)));
    }
}

/// The ids of one page of a model list.
fn ids(page: &Value) -> Vec<String> {
    let listed = page.get("data").and_then(Value::as_array).into_iter().flatten();
    listed.filter_map(|m| m.get("id").and_then(Value::as_str)).map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

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

    fn anthropic(base: String) -> ProviderConfig {
        ProviderConfig { anthropic: Some(("k".into(), base)), openai_compat: None }
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
        spawn(&anthropic(serve(app).await), &catalog);
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
        spawn(&anthropic(serve(app).await), &catalog);
        until(|| hits.load(Ordering::SeqCst) > 0).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(catalog.supports("claude-haiku-5-5"));
    }

    /// A Models API that answers every read with `page`, how many reads it
    /// answered, and the catalog the reads fill.
    async fn served(page: serde_json::Value) -> (Arc<AtomicUsize>, ModelCatalog) {
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        let answer = move || {
            counted.fetch_add(1, Ordering::SeqCst);
            let page = page.clone();
            async move { Json(page) }
        };
        let catalog = ModelCatalog::default_catalog();
        spawn(&anthropic(serve(Router::new().route("/v1/models", get(answer))).await), &catalog);
        (hits, catalog)
    }

    #[tokio::test]
    async fn an_empty_read_keeps_the_built_in_models() {
        let (hits, catalog) = served(json!({"data": [], "has_more": false})).await;
        until(|| hits.load(Ordering::SeqCst) > 0).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(catalog.supports("claude-haiku-5-5"));
    }

    #[tokio::test]
    async fn a_next_page_with_no_new_cursor_keeps_the_built_in_models() {
        let repeated = json!({"data": [{"id": "claude-new-1"}], "has_more": true, "last_id": "claude-new-1"});
        let missing = json!({"data": [{"id": "claude-new-1"}], "has_more": true});
        for (page, reads) in [(repeated, 2), (missing, 1)] {
            let (hits, catalog) = served(page).await;
            until(|| hits.load(Ordering::SeqCst) >= reads).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(hits.load(Ordering::SeqCst), reads, "no read past a page with no new cursor");
            assert!(catalog.supports("claude-haiku-5-5"), "the built-in list stays");
            assert!(!catalog.supports("claude-new-1"), "a partial list is not installed");
        }
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
        let base = format!("{}/api/v1", serve(app).await);
        spawn(&ProviderConfig { anthropic: None, openai_compat: Some(("k".into(), base)) }, &catalog);
        until(|| catalog.supports("openai/gpt-6-luna")).await;
        assert!(catalog.supports("qwen/qwen3.7-flash:free"));
        assert!(!catalog.supports("qwen/qwen3.7-flash"), "a variant only as listed");
        assert!(catalog.supports("claude-haiku-5-5"), "Anthropic's list stays");
    }

    #[tokio::test]
    async fn a_list_with_no_vendor_model_id_keeps_the_earlier_list() {
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        let answer = move || {
            counted.fetch_add(1, Ordering::SeqCst);
            async { Json(json!({"data": [{"id": "claude-haiku-5-5"}]})) }
        };
        let base = format!("{}/api/v1", serve(Router::new().route("/api/v1/models", get(answer))).await);
        let catalog = ModelCatalog::default_catalog();
        catalog.replace_openai_compat(["openai/gpt-6-luna".to_owned()]);
        spawn(&ProviderConfig { anthropic: None, openai_compat: Some(("k".into(), base)) }, &catalog);
        until(|| hits.load(Ordering::SeqCst) > 0).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(catalog.supports("openai/gpt-6-luna"), "the earlier list stays");
    }
}
