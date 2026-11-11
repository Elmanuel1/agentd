//! List envelopes + cursor pagination.
//!
//! Live-probed: Managed Agents lists return `{"data":[…], "next_page":"page_…"}`,
//! where `next_page` is an opaque cursor, omitted on the last page (the GA
//! Files and Skills lists send `null` instead). The next page is fetched with
//! `?page=<cursor>&limit=N`; `limit` defaults to 20, and each list's
//! [`ListSpec`] sets its bounds and messages. The cursor value is opaque to
//! clients, so agentd mints its own `page_…` tokens.

use std::collections::HashMap;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Serialize;

use crate::error::ApiError;

pub mod filters;
mod id_cursor;
mod query;
mod spec;

pub use id_cursor::{IdCursor, IdPage};
pub use query::ListQuery;
pub use spec::{ListSpec, Messages, UnknownParams};

/// Default page size when `limit` is omitted.
pub const DEFAULT_LIMIT: usize = 20;
/// Maximum allowed `limit`.
pub const MAX_LIMIT: usize = 100;

/// A paginated Managed Agents list response: `{data, next_page?}`.
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    /// The items on this page.
    pub data: Vec<T>,
    /// Opaque cursor for the next page; omitted when there are no more results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<String>,
    /// Cursor for the previous page (bidirectional lists only); omitted on
    /// the first page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_page: Option<String>,
}

impl<T> Page<T> {
    /// Slice `all` (a fully-ordered result set) to the requested page and set
    /// `next_page` when more remain.
    #[must_use]
    pub fn paginate(all: Vec<T>, page: &Pagination) -> Self {
        let total = all.len();
        let start = page.offset.min(total);
        let end = start.saturating_add(page.limit).min(total);
        let data: Vec<T> = all.into_iter().skip(start).take(page.limit).collect();
        let next_page = (end < total).then(|| encode_cursor(end));
        Self { data, next_page, prev_page: None }
    }

    /// Build a page from rows the store fetched DB-side with
    /// [`Pagination::fetch_limit`] + [`Pagination::sql_offset`]: the store asks
    /// for `limit + 1` rows, so a returned row beyond `limit` proves more remain
    /// without loading the whole table. Behaviourally identical to
    /// [`Self::paginate`] over the full set, but only `limit + 1` rows travel.
    #[must_use]
    pub fn from_fetched(mut rows: Vec<T>, page: &Pagination) -> Self {
        let has_more = rows.len() > page.limit;
        rows.truncate(page.limit);
        let next_page = has_more.then(|| encode_cursor(page.offset + page.limit));
        Self { data: rows, next_page, prev_page: None }
    }

    /// Add the cursor back to the previous page, for a bidirectional list.
    #[must_use]
    pub fn with_prev_page(mut self, page: &Pagination) -> Self {
        self.prev_page = (page.offset > 0).then(|| encode_cursor(page.offset.saturating_sub(page.limit)));
        self
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
            Some(cursor) => {
                decode_cursor(cursor).ok_or_else(|| ApiError::invalid_request("page: invalid cursor"))?
            }
        };
        Ok(Self { limit, offset })
    }

    /// SQL `LIMIT` to fetch: `limit + 1`. The extra row (if present) signals
    /// "more remain" without a second count query. Capped at `i64`.
    #[must_use]
    pub fn fetch_limit(&self) -> i64 {
        self.limit.saturating_add(1).min(i64::MAX as usize) as i64
    }

    /// SQL `OFFSET` for this page.
    #[must_use]
    pub fn sql_offset(&self) -> i64 {
        self.offset.min(i64::MAX as usize) as i64
    }

    /// Slice an already-materialized ordered set to this page's window,
    /// yielding up to `limit + 1` rows so [`Page::from_fetched`] can still
    /// detect "more". For stores that cannot push LIMIT/OFFSET into the query
    /// (e.g. a DynamoDB partition query returns the whole partition).
    #[must_use]
    pub fn window<T>(&self, rows: Vec<T>) -> Vec<T> {
        rows.into_iter().skip(self.offset).take(self.limit.saturating_add(1)).collect()
    }
}

/// The query flag the SDKs and `ant` send on every beta call, which live accepts.
pub(crate) const BETA: &str = "beta";

/// Reject any query param not in `allowed` (or [`BETA`]), matching the live 400
/// `Failed to parse request body: unknown field "X"`. Callers pass exactly the
/// params they read, so a valid param is never rejected.
pub fn reject_unknown_params(params: &HashMap<String, String>, allowed: &[&str]) -> Result<(), ApiError> {
    for key in params.keys() {
        if key != BETA && !allowed.contains(&key.as_str()) {
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

pub(crate) fn decode_cursor(cursor: &str) -> Option<usize> {
    let b64 = cursor.strip_prefix("page_")?;
    let bytes = URL_SAFE_NO_PAD.decode(b64).ok()?;
    std::str::from_utf8(&bytes).ok()?.parse().ok()
}

#[cfg(test)]
mod tests;
