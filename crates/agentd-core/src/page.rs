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
    #[must_use]
    pub fn of(data: Vec<T>) -> Self {
        Self {
            data,
            next_page: None,
        }
    }

    /// Slice `all` (a fully-ordered result set) to the requested page and set
    /// `next_page` when more remain.
    #[must_use]
    pub fn paginate(all: Vec<T>, page: &Pagination) -> Self {
        let total = all.len();
        let start = page.offset.min(total);
        let end = start.saturating_add(page.limit).min(total);
        let data: Vec<T> = all.into_iter().skip(start).take(page.limit).collect();
        let next_page = (end < total).then(|| encode_cursor(end));
        Self { data, next_page }
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
        Self {
            data: rows,
            next_page,
        }
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

    /// A plain list endpoint's controls: reject any param other than
    /// `limit`/`page`, then parse. Collapses the `reject_unknown_params(…) +
    /// from_params(…)` pair repeated at every simple list route.
    pub fn from_list_params(params: &HashMap<String, String>) -> Result<Self, ApiError> {
        reject_unknown_params(params, &["limit", "page"])?;
        Self::from_params(params)
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
        rows.into_iter()
            .skip(self.offset)
            .take(self.limit.saturating_add(1))
            .collect()
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
    /// Paginate a fully-materialized ordered set by id cursor (for backends
    /// that return the whole set). Prefer pushing the cursor DB-side via
    /// [`IdCursor`] + [`Self::from_page`] on the default store.
    pub fn paginate(
        all: Vec<T>,
        params: &HashMap<String, String>,
        id_of: impl Fn(&T) -> &str,
    ) -> Result<Self, ApiError> {
        let cursor = IdCursor::from_params(params)?;
        let (data, has_more) = cursor.window(all, &id_of);
        Ok(Self::from_page(data, has_more, id_of))
    }

    /// Build the envelope from a page the store already sliced (DB-side keyset:
    /// fetch `limit + 1`, `has_more = fetched > limit`). Only the two cursor ids
    /// are owned, not one `String` per row.
    #[must_use]
    pub fn from_page(data: Vec<T>, has_more: bool, id_of: impl Fn(&T) -> &str) -> Self {
        let first_id = data.first().map(|t| id_of(t).to_owned());
        let last_id = data.last().map(|t| id_of(t).to_owned());
        Self {
            data,
            has_more,
            first_id,
            last_id,
        }
    }
}

/// Parsed Files-API id-cursor controls: `after_id`/`before_id` exclusive bounds
/// and a page `limit`. The default store resolves the bounds to sort keys and
/// pushes a keyset predicate into SQL; partition-scan backends fall back to
/// [`Self::window`] over the full set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdCursor {
    /// Exclusive lower bound: items strictly after this id in list order.
    pub after_id: Option<String>,
    /// Exclusive upper bound: items strictly before this id in list order.
    pub before_id: Option<String>,
    /// Page size (1..=100).
    pub limit: usize,
}

impl IdCursor {
    /// Parse `limit`/`after_id`/`before_id` (limit validated like every list).
    pub fn from_params(params: &HashMap<String, String>) -> Result<Self, ApiError> {
        Ok(Self {
            after_id: params.get("after_id").cloned(),
            before_id: params.get("before_id").cloned(),
            limit: Pagination::from_params(params)?.limit,
        })
    }

    /// SQL `LIMIT` to fetch: `limit + 1` (the extra row proves "more remain").
    #[must_use]
    pub fn fetch_limit(&self) -> i64 {
        self.limit.saturating_add(1).min(i64::MAX as usize) as i64
    }

    /// Apply the cursor to a fully-materialized ordered set, returning the page
    /// rows plus `has_more`. A missing `after_id`/`before_id` is a no-op bound
    /// (matches the Files API: an unknown cursor id does not clamp the page).
    #[must_use]
    pub fn window<T>(&self, all: Vec<T>, id_of: impl Fn(&T) -> &str) -> (Vec<T>, bool) {
        let start = match &self.after_id {
            Some(a) => all.iter().position(|t| id_of(t) == a).map_or(0, |p| p + 1),
            None => 0,
        };
        let end = match &self.before_id {
            Some(b) => all.iter().position(|t| id_of(t) == b).unwrap_or(all.len()),
            None => all.len(),
        };
        let window: Vec<T> = all
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i >= start && *i < end)
            .map(|(_, t)| t)
            .collect();
        let has_more = window.len() > self.limit;
        (window.into_iter().take(self.limit).collect(), has_more)
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
    fn id_cursor_window_bounds_and_has_more() {
        #[allow(clippy::ptr_arg)] // the &String matches window's `Fn(&T)` HRTB
        fn key(s: &String) -> &str {
            s.as_str()
        }
        // Items are ids "0".."9" in list order.
        let all: Vec<String> = (0..10).map(|n| n.to_string()).collect();

        // after_id excludes everything up to and including it; limit caps + flags more.
        let c = IdCursor { after_id: Some("2".into()), before_id: None, limit: 3 };
        let (data, more) = c.window(all.clone(), key);
        assert_eq!(data, vec!["3", "4", "5"]);
        assert!(more);

        // before_id is an exclusive upper bound.
        let c = IdCursor { after_id: Some("2".into()), before_id: Some("6".into()), limit: 10 };
        let (data, more) = c.window(all.clone(), key);
        assert_eq!(data, vec!["3", "4", "5"]);
        assert!(!more);

        // Unknown cursor id is a no-op bound (whole set, from the start).
        let c = IdCursor { after_id: Some("nope".into()), before_id: None, limit: 100 };
        let (data, more) = c.window(all, key);
        assert_eq!(data.len(), 10);
        assert!(!more);
    }

    #[test]
    fn from_fetched_matches_paginate_over_full_set() {
        // The store fetches `fetch_limit()` rows at `sql_offset()`; from_fetched
        // must produce the same page (data + next_page) as slicing the full set.
        let all: Vec<u8> = (0..25).collect();
        for offset in [0usize, 10, 20, 25, 40] {
            for limit in [1usize, 7, 20, 100] {
                let page = Pagination { limit, offset };
                let full = Page::paginate(all.clone(), &page);
                // Simulate the DB fetch: skip=offset, take=limit+1.
                let fetched: Vec<u8> = all
                    .iter()
                    .copied()
                    .skip(page.sql_offset() as usize)
                    .take(page.fetch_limit() as usize)
                    .collect();
                let db = Page::from_fetched(fetched, &page);
                assert_eq!(db.data, full.data, "data at off={offset} lim={limit}");
                assert_eq!(db.next_page, full.next_page, "cursor at off={offset} lim={limit}");
            }
        }
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
