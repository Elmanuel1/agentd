//! Per-list query rules. Each list endpoint declares one [`ListSpec`]: its
//! page-size bounds, the live validation messages of its API family, the
//! filter params it reads, and what it does with any other param.

use std::collections::HashMap;

use super::{decode_cursor, Pagination, DEFAULT_LIMIT};
use crate::error::ApiError;

/// How a list answers a query param it does not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownParams {
    /// 400 `Failed to parse request body: unknown field "X"` (Managed Agents).
    Reject,
    /// Ignored, except the listed legacy params, which get 400 `X: unknown field`
    /// (Files, Skills).
    Ignore(&'static [&'static str]),
}

/// The wording of a list's `limit` and `page` errors, which differs by API family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Messages {
    /// `limit: value must be greater than or equal to 1 and less than or equal to 100`;
    /// `page: invalid cursor`.
    ManagedAgents,
    /// `limit: Input should be greater than or equal to 1` (or `less than or equal
    /// to N`); `page: invalid cursor`.
    Files,
    /// `limit must be between 1 and N`; `page is not a valid page token.`
    Skills,
}

/// One list endpoint's query rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListSpec {
    /// Largest allowed `limit`.
    pub max_limit: usize,
    /// The filter params the endpoint reads, besides `limit` and `page`.
    pub filters: &'static [&'static str],
    /// What happens to any other param.
    pub unknown: UnknownParams,
    /// The error wording.
    pub messages: Messages,
}

impl ListSpec {
    /// A Managed Agents list (limit 1 to 100, unknown params rejected) reading `filters`.
    #[must_use]
    pub const fn managed(filters: &'static [&'static str]) -> Self {
        Self { max_limit: 100, filters, unknown: UnknownParams::Reject, messages: Messages::ManagedAgents }
    }

    /// Check the query and parse its `limit` and `page`.
    pub fn parse(&self, params: &HashMap<String, String>) -> Result<Pagination, ApiError> {
        for key in params.keys().map(String::as_str) {
            if key == "limit" || key == "page" || self.filters.contains(&key) {
                continue;
            }
            match self.unknown {
                UnknownParams::Reject => {
                    return Err(ApiError::invalid_request(format!(
                        "Failed to parse request body: unknown field \"{key}\""
                    )))
                }
                UnknownParams::Ignore(rejected) if rejected.contains(&key) => {
                    return Err(ApiError::invalid_request(format!("{key}: unknown field")))
                }
                UnknownParams::Ignore(_) => {}
            }
        }
        let limit = match params.get("limit") {
            None => DEFAULT_LIMIT,
            Some(raw) => match raw.parse::<usize>() {
                Ok(n) if (1..=self.max_limit).contains(&n) => n,
                parsed => return Err(self.limit_error(parsed.ok())),
            },
        };
        let offset = match params.get("page") {
            None => 0,
            Some(cursor) => decode_cursor(cursor).ok_or_else(|| self.page_error())?,
        };
        Ok(Pagination { limit, offset })
    }

    fn limit_error(&self, value: Option<usize>) -> ApiError {
        let max = self.max_limit;
        ApiError::invalid_request(match self.messages {
            Messages::ManagedAgents => {
                format!("limit: value must be greater than or equal to 1 and less than or equal to {max}")
            }
            Messages::Files if value.is_some_and(|n| n > max) => {
                format!("limit: Input should be less than or equal to {max}")
            }
            Messages::Files => "limit: Input should be greater than or equal to 1".to_owned(),
            Messages::Skills => format!("limit must be between 1 and {max}"),
        })
    }

    fn page_error(&self) -> ApiError {
        ApiError::invalid_request(match self.messages {
            Messages::ManagedAgents | Messages::Files => "page: invalid cursor",
            Messages::Skills => "page is not a valid page token.",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }

    #[test]
    fn managed_lists_reject_unknown_params_and_cap_at_100() {
        let spec = ListSpec::managed(&["include_archived"]);
        assert_eq!(spec.parse(&q(&[("include_archived", "true")])).unwrap().limit, 20);
        let err = spec.parse(&q(&[("bogus", "1")])).unwrap_err();
        assert_eq!(err.message, "Failed to parse request body: unknown field \"bogus\"");
        let err = spec.parse(&q(&[("limit", "101")])).unwrap_err();
        assert_eq!(
            err.message,
            "limit: value must be greater than or equal to 1 and less than or equal to 100"
        );
    }

    #[test]
    fn files_lists_ignore_unknown_params_but_not_legacy_ones() {
        // Live: files/21, 22, 23, 27, 28.
        let spec = ListSpec {
            max_limit: 1000,
            filters: &["ids"],
            unknown: UnknownParams::Ignore(&["after_id", "before_id"]),
            messages: Messages::Files,
        };
        assert_eq!(spec.parse(&q(&[("limit", "1000"), ("bogus", "1")])).unwrap().limit, 1000);
        assert_eq!(
            spec.parse(&q(&[("limit", "0")])).unwrap_err().message,
            "limit: Input should be greater than or equal to 1"
        );
        assert_eq!(
            spec.parse(&q(&[("limit", "1001")])).unwrap_err().message,
            "limit: Input should be less than or equal to 1000"
        );
        assert_eq!(spec.parse(&q(&[("after_id", "file_1")])).unwrap_err().message, "after_id: unknown field");
        assert_eq!(spec.parse(&q(&[("page", "garbage")])).unwrap_err().message, "page: invalid cursor");
    }

    #[test]
    fn skills_lists_use_the_skills_wording() {
        // Live: skills/40, 41, 42.
        let spec = ListSpec {
            max_limit: 1000,
            filters: &[],
            unknown: UnknownParams::Ignore(&[]),
            messages: Messages::Skills,
        };
        assert_eq!(spec.parse(&q(&[("limit", "101")])).unwrap().limit, 101);
        assert_eq!(
            spec.parse(&q(&[("limit", "0")])).unwrap_err().message,
            "limit must be between 1 and 1000"
        );
        assert_eq!(spec.parse(&q(&[("page", "x")])).unwrap_err().message, "page is not a valid page token.");
    }
}
