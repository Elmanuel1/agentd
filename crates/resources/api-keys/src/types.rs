//! A workspace API key as a listing shows it, without its secret.

use shared::{OrgId, Resource, Timestamp, WorkspaceId};

/// Marker for `apikey_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct ApiKeyResource;
impl Resource for ApiKeyResource {
    const PREFIX: &'static str = "apikey";
    const LABEL: &'static str = "API key";
}

/// One workspace API key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    /// `apikey_…`.
    pub id: String,
    /// The organization it acts for.
    pub org: OrgId,
    /// The workspace it acts in.
    pub workspace: WorkspaceId,
    /// A label for people.
    pub name: String,
    /// Its first 16 and last 4 characters.
    pub partial_key_hint: String,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it was revoked; it resolves to no one from then on.
    pub revoked_at: Option<Timestamp>,
}
