//! The body of every delete: `{"id": "...", "type": "<resource>_deleted"}`.

use serde::Serialize;

/// A deleted resource's id and its `<resource>_deleted` type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Deleted {
    /// The deleted resource's id.
    pub id: String,
    /// `<resource>_deleted`.
    #[serde(rename = "type")]
    pub tag: &'static str,
}

impl Deleted {
    /// `id` deleted, of type `tag`.
    pub fn new(id: impl Into<String>, tag: &'static str) -> Self {
        Self { id: id.into(), tag }
    }
}
