//! The models this deployment accepts, and the live error for one it does
//! not.

use std::sync::{Arc, RwLock};

use crate::ApiError;
use crate::prices::undated;

/// `` `model.id`: model "gpt-4": model is not supported ``
pub fn validate_model_supported<'a>(model_id: &'a str, catalog: &ModelCatalog) -> Result<&'a str, ApiError> {
    if catalog.supports(model_id) {
        Ok(model_id)
    } else {
        Err(ApiError::invalid_request(format!("`model.id`: model \"{model_id}\": model is not supported")))
    }
}

/// The ids Anthropic's `GET /v1/models` lists on 2026-11-21. Live's agent
/// create accepts exactly these and the undated alias of each dated one.
const ANTHROPIC: [&str; 15] = [
    "claude-fable-5-2",
    "claude-haiku-5-5",
    "claude-sonnet-5-5",
    "claude-opus-5-5",
    "claude-fable-5-1",
    "claude-opus-5",
    "claude-sonnet-5",
    "claude-fable-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-sonnet-4-6",
    "claude-opus-4-6",
    "claude-opus-4-5-20251101",
    "claude-haiku-4-5-20251001",
    "claude-sonnet-4-5-20250929",
];

/// The model ids this deployment accepts: Anthropic's, the built-in list
/// until [`ModelCatalog::replace_anthropic`] installs the Models API's, and
/// the `AGENTD_MODELS` extras. Every clone shares one Anthropic list.
#[derive(Debug, Clone)]
pub struct ModelCatalog {
    anthropic: Arc<RwLock<Vec<String>>>,
    extra: Vec<String>,
}

impl ModelCatalog {
    /// The built-in Anthropic list and no extras.
    #[must_use]
    pub fn default_catalog() -> Self {
        let anthropic = with_aliases(ANTHROPIC.map(String::from));
        Self { anthropic: Arc::new(RwLock::new(anthropic)), extra: Vec::new() }
    }

    /// The catalog with `extra` ids accepted as well.
    #[must_use]
    pub fn with(mut self, extra: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.extra.extend(extra.into_iter().map(Into::into));
        self
    }

    /// Accept exactly the `listed` Anthropic ids from now on, each dated one
    /// with its undated alias.
    pub fn replace_anthropic(&self, listed: impl IntoIterator<Item = String>) {
        let ids = with_aliases(listed);
        if let Ok(mut current) = self.anthropic.write() {
            *current = ids;
        }
    }

    /// Whether a model id is accepted.
    #[must_use]
    pub fn supports(&self, id: &str) -> bool {
        let anthropic = self.anthropic.read().is_ok_and(|ids| ids.iter().any(|m| m == id));
        anthropic || self.extra.iter().any(|m| m == id)
    }
}

fn with_aliases(listed: impl IntoIterator<Item = String>) -> Vec<String> {
    listed
        .into_iter()
        .flat_map(|id| {
            let alias = Some(undated(&id)).filter(|a| *a != id).map(str::to_owned);
            std::iter::once(id).chain(alias)
        })
        .collect()
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

    #[test]
    fn the_built_in_list_accepts_what_live_accepts() {
        let cat = ModelCatalog::default_catalog();
        for id in ["claude-haiku-5-5", "claude-haiku-4-5", "claude-haiku-4-5-20251001", "claude-sonnet-4-5"] {
            assert!(cat.supports(id), "{id}");
        }
        for id in [
            "claude-opus-4-1",
            "claude-opus-4-20250514",
            "claude-3-haiku-20240307",
            "claude-haiku-5-5-20990101",
            "claude-mythos-5-2",
            "CLAUDE-HAIKU-5-5",
            "anthropic/claude-haiku-4.5",
        ] {
            assert!(!cat.supports(id), "{id}");
        }
    }

    #[test]
    fn a_replaced_list_is_what_every_clone_accepts() {
        let cat = ModelCatalog::default_catalog().with(["openai/gpt-6-luna"]);
        let clone = cat.clone();
        cat.replace_anthropic(["claude-new-1".to_owned(), "claude-old-1-20250101".to_owned()]);
        for id in ["claude-new-1", "claude-old-1-20250101", "claude-old-1", "openai/gpt-6-luna"] {
            assert!(clone.supports(id), "{id}");
        }
        assert!(!clone.supports("claude-haiku-5-5"), "a model the list lacks");
    }
}
