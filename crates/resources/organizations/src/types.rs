//! Admin API wire objects.

use serde::Serialize;
use shared::AdminContext;

/// `GET /v1/organizations/me`: `{id, type: "organization", name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Organization {
    /// The organization id.
    pub id: String,
    /// Always `organization`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The organization's name, which is its id.
    pub name: String,
}

impl Organization {
    /// The organization `ctx`'s admin key acts for.
    #[must_use]
    pub fn of(ctx: &AdminContext) -> Self {
        Self { id: ctx.org.to_string(), kind: "organization", name: ctx.org.to_string() }
    }
}
