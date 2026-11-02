//! Typed list filters read from a list's query, with pydantic-style messages.

use std::collections::HashMap;

use crate::error::ApiError;
use crate::Timestamp;

/// A `created_at[gt|gte|lt|lte]` range; each bound optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CreatedRange {
    /// `created_at[gt]`.
    pub gt: Option<Timestamp>,
    /// `created_at[gte]`.
    pub gte: Option<Timestamp>,
    /// `created_at[lt]`.
    pub lt: Option<Timestamp>,
    /// `created_at[lte]`.
    pub lte: Option<Timestamp>,
}

impl CreatedRange {
    /// The `created_at[...]` bounds in `params`.
    pub fn from_query(params: &HashMap<String, String>) -> Result<Self, ApiError> {
        Ok(Self {
            gt: time_param(params, "created_at[gt]")?,
            gte: time_param(params, "created_at[gte]")?,
            lt: time_param(params, "created_at[lt]")?,
            lte: time_param(params, "created_at[lte]")?,
        })
    }

    /// Whether `t` is inside every bound.
    #[must_use]
    pub fn contains(&self, t: Timestamp) -> bool {
        let t = t.as_datetime();
        self.gt.is_none_or(|b| t > b.as_datetime())
            && self.gte.is_none_or(|b| t >= b.as_datetime())
            && self.lt.is_none_or(|b| t < b.as_datetime())
            && self.lte.is_none_or(|b| t <= b.as_datetime())
    }
}

/// A boolean query param; `false` when left out.
pub fn bool_param(params: &HashMap<String, String>, key: &str) -> Result<bool, ApiError> {
    match params.get(key).map(String::as_str) {
        None => Ok(false),
        Some("true" | "1") => Ok(true),
        Some("false" | "0") => Ok(false),
        Some(_) => Err(ApiError::invalid_request(format!("{key}: Input should be a valid boolean"))),
    }
}

/// An RFC 3339 timestamp query param.
pub fn time_param(params: &HashMap<String, String>, key: &str) -> Result<Option<Timestamp>, ApiError> {
    params
        .get(key)
        .map(|raw| {
            serde_json::from_value::<Timestamp>(serde_json::Value::String(raw.clone()))
                .map_err(|_| ApiError::invalid_request(format!("{key}: Input should be a valid datetime")))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }

    #[test]
    fn booleans_and_ranges_parse_or_name_the_param() {
        assert!(bool_param(&q(&[("include_archived", "true")]), "include_archived").unwrap());
        assert!(!bool_param(&q(&[]), "include_archived").unwrap());
        assert_eq!(
            bool_param(&q(&[("include_archived", "yes!")]), "include_archived").unwrap_err().message,
            "include_archived: Input should be a valid boolean"
        );
        let range = CreatedRange::from_query(&q(&[("created_at[gte]", "2026-01-01T00:00:00Z")])).unwrap();
        let t = |s: &str| serde_json::from_value::<Timestamp>(serde_json::Value::String(s.into())).unwrap();
        assert!(range.contains(t("2026-01-01T00:00:00Z")) && !range.contains(t("2025-12-31T23:59:59Z")));
        assert_eq!(
            CreatedRange::from_query(&q(&[("created_at[lt]", "nope")])).unwrap_err().message,
            "created_at[lt]: Input should be a valid datetime"
        );
    }
}
