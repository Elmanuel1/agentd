//! The models this deployment accepts, and the live error for one it does
//! not.

use crate::ApiError;

/// `` `model.id`: model "gpt-4": model is not supported ``
pub fn validate_model_supported<'a>(model_id: &'a str, catalog: &ModelCatalog) -> Result<&'a str, ApiError> {
    if catalog.supports(model_id) {
        Ok(model_id)
    } else {
        Err(ApiError::invalid_request(format!("`model.id`: model \"{model_id}\": model is not supported")))
    }
}

/// The set of model IDs this deployment will accept.
#[derive(Debug, Clone)]
pub struct ModelCatalog {
    models: Vec<String>,
}

impl ModelCatalog {
    /// Build from an explicit list.
    #[must_use]
    pub fn new(models: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { models: models.into_iter().map(Into::into).collect() }
    }

    /// Default catalog mirroring currently-served Anthropic model aliases.
    #[must_use]
    pub fn default_catalog() -> Self {
        Self::new([
            "claude-fable-5",
            "claude-sonnet-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-opus-4-5",
            "claude-sonnet-4-6",
            "claude-sonnet-4-5",
            "claude-haiku-4-5",
        ])
    }

    /// The catalog with `extra` ids accepted as well.
    #[must_use]
    pub fn with(mut self, extra: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.models.extend(extra.into_iter().map(Into::into));
        self
    }

    /// Whether a model id is accepted.
    #[must_use]
    pub fn supports(&self, id: &str) -> bool {
        self.models.iter().any(|m| m == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_catalog_exact_live_string() {
        let cat = ModelCatalog::default_catalog();
        let e = validate_model_supported("gpt-4", &cat).unwrap_err();
        assert_eq!(e.message, "`model.id`: model \"gpt-4\": model is not supported");
        assert!(validate_model_supported("claude-sonnet-4-6", &cat).is_ok());
    }
}
