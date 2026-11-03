//! A list's whole query string, so repeated array params
//! (`statuses[]=idle&statuses[]=running`, the SDKs' bracket format) all reach
//! the route.

use std::collections::HashMap;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::error::ApiError;
use crate::extract::RouteError;

/// The query's `(key, value)` pairs, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListQuery(Vec<(String, String)>);

impl ListQuery {
    /// Parse a raw query string.
    pub fn parse(raw: &str) -> Result<Self, ApiError> {
        serde_urlencoded::from_str(raw)
            .map(Self)
            .map_err(|_| ApiError::invalid_request("Failed to parse request query"))
    }

    /// Single-valued params by key (the last value of a repeated key), for
    /// [`super::ListSpec::parse`].
    #[must_use]
    pub fn map(&self) -> HashMap<String, String> {
        self.0.iter().cloned().collect()
    }

    /// Every value of array param `key`, sent as `key[]` or `key`.
    #[must_use]
    pub fn all(&self, key: &str) -> Vec<&str> {
        let bracketed = format!("{key}[]");
        self.0.iter().filter(|(k, _)| *k == key || *k == bracketed).map(|(_, v)| v.as_str()).collect()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for ListQuery {
    type Rejection = RouteError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::parse(parts.uri.query().unwrap_or_default())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_bracketed_params_are_all_kept() {
        let q = ListQuery::parse("statuses%5B%5D=idle&statuses%5B%5D=running&limit=5").unwrap();
        assert_eq!(q.all("statuses"), ["idle", "running"]);
        assert_eq!(q.map().get("limit").map(String::as_str), Some("5"));
    }
}
