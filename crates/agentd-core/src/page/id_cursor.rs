use std::collections::HashMap;

use serde::Serialize;

use super::Pagination;
use crate::error::ApiError;

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
        Self { data, has_more, first_id, last_id }
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
        let window: Vec<T> =
            all.into_iter().enumerate().filter(|(i, _)| *i >= start && *i < end).map(|(_, t)| t).collect();
        let has_more = window.len() > self.limit;
        (window.into_iter().take(self.limit).collect(), has_more)
    }
}
