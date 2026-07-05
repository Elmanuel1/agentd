//! List envelopes + cursor pagination.
//!
//! Live-probed (managed-agents-2026-04-01, 2026-07-05): list endpoints return
//! `{"data":[…], "next_page":"page_…"}` where `next_page` is an opaque cursor
//! present ONLY when more results exist (omitted otherwise). The next page is
//! fetched with `?page=<cursor>&limit=N`; `limit` defaults to 20 and must be
//! 1–100 (else 400). The cursor value is opaque to clients, so agentd mints its
//! own `page_…` tokens.

use std::collections::HashMap;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Serialize;

use crate::error::ApiError;

/// Default page size when `limit` is omitted.
pub const DEFAULT_LIMIT: usize = 20;
/// Maximum allowed `limit`.
pub const MAX_LIMIT: usize = 100;

/// A paginated list response — `{data, next_page?}`.
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    /// The items on this page.
    pub data: Vec<T>,
    /// Opaque cursor for the next page; omitted when there are no more results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<String>,
}

impl<T> Page<T> {
    /// Bare `{"data":[…]}` (no pagination) — for non-list responses like the
    /// echo of just-sent events.
    pub fn of(data: Vec<T>) -> Self {
        Self {
            data,
            next_page: None,
        }
    }

    /// Slice `all` (a fully-ordered result set) to the requested page and set
    /// `next_page` when more remain.
    pub fn paginate(all: Vec<T>, page: &Pagination) -> Self {
        let total = all.len();
        let start = page.offset.min(total);
        let end = start.saturating_add(page.limit).min(total);
        let data: Vec<T> = all.into_iter().skip(start).take(page.limit).collect();
        let next_page = (end < total).then(|| encode_cursor(end));
        Self { data, next_page }
    }
}

/// Parsed pagination controls: a page size and an offset into the ordered set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    /// Page size (1..=100).
    pub limit: usize,
    /// Offset into the ordered result set (decoded from the `page` cursor).
    pub offset: usize,
}

impl Pagination {
    /// Parse `limit` + `page` from query params, matching the live validation
    /// (`limit: value must be greater than or equal to 1 and less than or
    /// equal to 100`).
    pub fn from_params(params: &HashMap<String, String>) -> Result<Self, ApiError> {
        let limit = match params.get("limit") {
            None => DEFAULT_LIMIT,
            Some(raw) => {
                let n = raw.parse::<usize>().unwrap_or(0);
                if !(1..=MAX_LIMIT).contains(&n) {
                    return Err(ApiError::invalid_request(
                        "limit: value must be greater than or equal to 1 and less than or equal to 100",
                    ));
                }
                n
            }
        };
        let offset = match params.get("page") {
            None => 0,
            Some(cursor) => decode_cursor(cursor)
                .ok_or_else(|| ApiError::invalid_request("page: invalid cursor"))?,
        };
        Ok(Self { limit, offset })
    }
}

/// The **standard Anthropic Files API** list envelope — `{data, has_more,
/// first_id, last_id}` (distinct from managed-agents' `next_page`). `/v1/files`
/// is the shared Files API and paginates by id cursor (`after_id`/`before_id`).
#[derive(Debug, Clone, Serialize)]
pub struct IdPage<T> {
    /// The items on this page.
    pub data: Vec<T>,
    /// Whether more results follow.
    pub has_more: bool,
    /// Id of the first item on this page (null when empty).
    pub first_id: Option<String>,
    /// Id of the last item on this page (null when empty).
    pub last_id: Option<String>,
}

impl<T> IdPage<T> {
    /// Paginate an ordered set by id cursor. `id_of` extracts each item's id;
    /// `after_id`/`before_id` are exclusive bounds and `limit` (1..=100,
    /// default 20) caps the page.
    pub fn paginate(
        all: Vec<T>,
        params: &HashMap<String, String>,
        id_of: impl Fn(&T) -> String,
    ) -> Result<Self, ApiError> {
        let limit = Pagination::from_params(params)?.limit;
        let ids: Vec<String> = all.iter().map(&id_of).collect();
        let start = match params.get("after_id") {
            Some(a) => ids.iter().position(|i| i == a).map_or(0, |p| p + 1),
            None => 0,
        };
        let end_bound = match params.get("before_id") {
            Some(b) => ids.iter().position(|i| i == b).unwrap_or(all.len()),
            None => all.len(),
        };
        let window: Vec<T> = all
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i >= start && *i < end_bound)
            .map(|(_, t)| t)
            .collect();
        let has_more = window.len() > limit;
        let data: Vec<T> = window.into_iter().take(limit).collect();
        let first_id = data.first().map(&id_of);
        let last_id = data.last().map(&id_of);
        Ok(Self {
            data,
            has_more,
            first_id,
            last_id,
        })
    }
}

/// Reject any query param not in `allowed`, matching the live 400
/// `Failed to parse request body: unknown field "X"`. Callers pass exactly the
/// params they read, so a valid param is never rejected.
pub fn reject_unknown_params(
    params: &HashMap<String, String>,
    allowed: &[&str],
) -> Result<(), ApiError> {
    for key in params.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ApiError::invalid_request(format!(
                "Failed to parse request body: unknown field \"{key}\""
            )));
        }
    }
    Ok(())
}

fn encode_cursor(offset: usize) -> String {
    format!("page_{}", URL_SAFE_NO_PAD.encode(offset.to_string()))
}

fn decode_cursor(cursor: &str) -> Option<usize> {
    let b64 = cursor.strip_prefix("page_")?;
    let bytes = URL_SAFE_NO_PAD.decode(b64).ok()?;
    std::str::from_utf8(&bytes).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_page_matches_live_shape() {
        // Live: an empty list is {"data":[]} (next_page omitted).
        let p: Page<u8> = Page::of(vec![]);
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"data":[]}"#);
    }

    #[test]
    fn paginate_sets_next_page_only_when_more() {
        let all: Vec<u8> = (0..25).collect();
        let first = Page::paginate(all.clone(), &Pagination { limit: 20, offset: 0 });
        assert_eq!(first.data.len(), 20);
        let cursor = first.next_page.clone().expect("more results → cursor present");
        assert!(cursor.starts_with("page_"));

        // round-trip the cursor to fetch page 2
        let mut params = HashMap::new();
        params.insert("page".to_string(), cursor);
        let p2 = Pagination::from_params(&params).unwrap();
        let second = Page::paginate(all, &p2);
        assert_eq!(second.data, (20..25).collect::<Vec<u8>>());
        assert!(second.next_page.is_none(), "last page omits next_page");
    }

    #[test]
    fn limit_validation_matches_live_message() {
        let mut params = HashMap::new();
        params.insert("limit".to_string(), "0".to_string());
        let err = Pagination::from_params(&params).unwrap_err();
        assert_eq!(
            err.message,
            "limit: value must be greater than or equal to 1 and less than or equal to 100"
        );
        params.insert("limit".to_string(), "101".to_string());
        assert!(Pagination::from_params(&params).is_err());
        params.insert("limit".to_string(), "100".to_string());
        assert_eq!(Pagination::from_params(&params).unwrap().limit, 100);
    }
}
