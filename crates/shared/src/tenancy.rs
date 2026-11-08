//! Two-level tenancy: Organization ⊃ Workspace.
//!
//! Live-verified: environments are org-scoped; agents/vaults/memory/sessions/
//! deployments are workspace-scoped (bound to the API key's workspace,
//! invisible on the wire). Every authenticated request resolves to an
//! [`AuthContext`] carrying both.

use serde::{Deserialize, Serialize};

/// Organization identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrgId(String);

/// Workspace identifier (`wrkspc_…` on the admin API).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(String);

impl OrgId {
    /// Wrap a raw org id.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    /// Borrow the raw id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl WorkspaceId {
    /// Wrap a raw workspace id.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    /// Borrow the raw id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OrgId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The resolved identity of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    /// The organization the API key belongs to.
    pub org: OrgId,
    /// The workspace the API key is bound to.
    pub workspace: WorkspaceId,
    /// The API key id (`apikey_…`) — used for audit actors.
    pub api_key_id: String,
}

/// The resolved identity of an admin request: an organization, no workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminContext {
    /// The organization the admin key belongs to.
    pub org: OrgId,
    /// The admin key id (`adminkey_…`).
    pub admin_key_id: String,
}

impl AuthContext {
    /// Audit actor for mutations performed under this context.
    pub fn actor(&self) -> crate::Actor {
        crate::Actor::ApiActor { api_key_id: self.api_key_id.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_types() {
        // Compile-time property: OrgId and WorkspaceId cannot be swapped.
        fn takes_org(_: &OrgId) {}
        let org = OrgId::new("org_a");
        takes_org(&org);
        // takes_org(&WorkspaceId::new("w")); // ← wrong type, does not compile, by design
    }

    #[test]
    fn actor_from_context() {
        let ctx = AuthContext {
            org: OrgId::new("org_a"),
            workspace: WorkspaceId::new("wrkspc_01"),
            api_key_id: "apikey_01".into(),
        };
        assert_eq!(ctx.actor(), crate::Actor::ApiActor { api_key_id: "apikey_01".into() });
    }
}
