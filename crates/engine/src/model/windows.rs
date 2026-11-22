//! Each model's context window, read from its provider's model list (C4):
//! the Messages API's `GET /v1/models/{id}` `max_input_tokens`, and an
//! OpenAI-compatible list's `context_length` (OpenRouter's `GET /models`).

use serde_json::Value;

use super::ModelError;
use super::anthropic::AnthropicModel;
use super::openai::OpenAiCompatModel;

pub(super) async fn get(req: reqwest::RequestBuilder) -> Result<Value, ModelError> {
    let resp = req.send().await.map_err(|e| ModelError::Retryable(e.to_string()))?;
    let status = resp.status().as_u16();
    let retry_after = ModelError::retry_after(resp.headers());
    let text = resp.text().await.map_err(|e| ModelError::Retryable(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(ModelError::from_status(status, text, retry_after));
    }
    serde_json::from_str(&text).map_err(|e| ModelError::Retryable(e.to_string()))
}

fn missing(model: &str) -> ModelError {
    ModelError::Fatal(format!("the provider lists no context window for model `{model}`"))
}

impl AnthropicModel {
    /// `max_input_tokens` of `model`.
    pub(super) async fn read_window(&self, model: &str) -> Result<u64, ModelError> {
        let v = get(self.get(&format!("/v1/models/{model}"))).await?;
        v.get("max_input_tokens").and_then(Value::as_u64).ok_or_else(|| missing(model))
    }
}

impl OpenAiCompatModel {
    /// `context_length` of `model` in the provider's list.
    pub(super) async fn read_window(&self, model: &str) -> Result<u64, ModelError> {
        let v = get(self.get("/models")).await?;
        let listed = v.get("data").and_then(Value::as_array).into_iter().flatten();
        listed
            .filter(|m| m.get("id").and_then(Value::as_str) == Some(model))
            .find_map(|m| m.get("context_length").and_then(Value::as_u64))
            .ok_or_else(|| missing(model))
    }
}

#[cfg(test)]
mod tests {
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;

    use crate::model::{ModelClient, ModelRouter, ProviderConfig};

    #[tokio::test]
    async fn each_provider_s_list_gives_the_window_once() {
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = hits.clone();
        let app = Router::new()
            .route(
                "/v1/models/claude-haiku-4-5",
                get(move || {
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async { Json(json!({"id": "claude-haiku-4-5", "max_input_tokens": 200000, "max_tokens": 64000})) }
                }),
            )
            .route(
                "/api/v1/models",
                get(|| async { Json(json!({"data": [{"id": "openai/gpt-x", "context_length": 400000}]})) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        let router = ModelRouter::from_config(&ProviderConfig {
            anthropic: Some(("k".into(), base.clone())),
            openai_compat: Some(("k".into(), format!("{base}/api/v1"))),
        })
        .unwrap();
        assert_eq!(router.window("claude-haiku-4-5").await.unwrap(), 200_000);
        assert_eq!(router.window("claude-haiku-4-5").await.unwrap(), 200_000);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1, "read once per process");
        assert_eq!(router.window("openai/gpt-x").await.unwrap(), 400_000);
        assert!(router.window("openai/unlisted").await.is_err(), "a model with no window fails");
    }
}
