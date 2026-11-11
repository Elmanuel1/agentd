//! Picks the provider for each request from the model id: OpenAI-compatible
//! for `vendor/model` ids (OpenRouter style), and the Messages API for
//! everything else.

use std::sync::Arc;

use async_trait::async_trait;

use super::anthropic::AnthropicModel;
use super::openai::OpenAiCompatModel;
use super::{ModelClient, ModelError, ModelRequest, ModelResponse};

/// The providers config enables. Its `Debug` leaves the keys out.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProviderConfig {
    /// `(api key, base url)` for the Messages API.
    pub anthropic: Option<(String, String)>,
    /// `(api key, base url)` for an OpenAI-compatible endpoint.
    pub openai_compat: Option<(String, String)>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("anthropic", &self.anthropic.as_ref().map(|(_, url)| url))
            .field("openai_compat", &self.openai_compat.as_ref().map(|(_, url)| url))
            .finish()
    }
}

/// One client per provider.
#[derive(Clone, Default)]
pub struct ModelRouter {
    anthropic: Option<Arc<dyn ModelClient>>,
    openai_compat: Option<Arc<dyn ModelClient>>,
}

impl std::fmt::Debug for ModelRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRouter")
            .field("anthropic", &self.anthropic.is_some())
            .field("openai_compat", &self.openai_compat.is_some())
            .finish()
    }
}

impl ModelRouter {
    /// Build the clients `config` enables.
    pub fn from_config(config: &ProviderConfig) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            anthropic: config.anthropic.as_ref().map(|(key, url)| {
                Arc::new(AnthropicModel::new(http.clone(), key, url)) as Arc<dyn ModelClient>
            }),
            openai_compat: config.openai_compat.as_ref().map(|(key, url)| {
                Arc::new(OpenAiCompatModel::new(http.clone(), key, url)) as Arc<dyn ModelClient>
            }),
        })
    }

    fn pick(&self, model: &str) -> Result<&Arc<dyn ModelClient>, ModelError> {
        let (chosen, provider) = match model.contains('/') {
            true => (self.openai_compat.as_ref(), "OpenAI-compatible"),
            false => (self.anthropic.as_ref(), "Anthropic"),
        };
        chosen.ok_or_else(|| {
            ModelError::Fatal(format!("no {provider} provider is configured for model `{model}`"))
        })
    }
}

#[async_trait]
impl ModelClient for ModelRouter {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.pick(&req.model)?.complete(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_model_ids_go_to_the_openai_compatible_provider() {
        let router = ModelRouter::from_config(&ProviderConfig {
            openai_compat: Some(("k".into(), "https://openrouter.ai/api/v1".into())),
            ..ProviderConfig::default()
        })
        .unwrap();
        assert!(router.pick("openai/gpt-6-luna").is_ok());
        let err = router.pick("claude-haiku-4-5").err().unwrap();
        assert_eq!(
            err,
            ModelError::Fatal("no Anthropic provider is configured for model `claude-haiku-4-5`".into())
        );
    }
}
