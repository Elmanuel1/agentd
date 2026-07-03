//! List envelopes.
//!
//! Live lists returned `{"data":[…]}` (agents, deployment_runs,
//! memory_versions). Pagination fields appear only when populated, so they
//! are optional and skipped when absent.

use serde::Serialize;

/// A paginated list response.
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    /// The items.
    pub data: Vec<T>,
    /// Whether more pages exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    /// ID of the first item on this page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_id: Option<String>,
    /// ID of the last item on this page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_id: Option<String>,
}

impl<T> Page<T> {
    /// Bare `{"data":[…]}` page, matching the live minimal shape.
    pub fn of(data: Vec<T>) -> Self {
        Self {
            data,
            has_more: None,
            first_id: None,
            last_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_page_matches_live_shape() {
        // Live probe: GET /v1/agents → {"data":[]}
        let p: Page<u8> = Page::of(vec![]);
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"data":[]}"#);
    }

    #[test]
    fn pagination_fields_only_when_present() {
        let p = Page {
            data: vec![1],
            has_more: Some(false),
            first_id: Some("x_01".into()),
            last_id: Some("x_01".into()),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["has_more"], false);
        assert_eq!(v["first_id"], "x_01");
    }
}
