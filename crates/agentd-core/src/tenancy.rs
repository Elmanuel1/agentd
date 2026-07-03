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
pub struct OrgId(pub String);

/// Workspace identifier (`wrkspc_…` on the admin API).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub String);

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

impl AuthContext {
    /// Audit actor for mutations performed under this context.
    pub fn actor(&self) -> crate::Actor {
        crate::Actor::ApiActor {
            api_key_id: self.api_key_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_types() {
        // Compile-time property: OrgId and WorkspaceId cannot be swapped.
        fn takes_org(_: &OrgId) {}
        let org = OrgId("org_a".into());
        takes_org(&org);
        // takes_org(&WorkspaceId("w".into())); // ← does not compile, by design
    }

    #[test]
    fn actor_from_context() {
        let ctx = AuthContext {
            org: OrgId("org_a".into()),
            workspace: WorkspaceId("wrkspc_01".into()),
            api_key_id: "apikey_01".into(),
        };
        assert_eq!(
            ctx.actor(),
            crate::Actor::ApiActor {
                api_key_id: "apikey_01".into()
            }
        );
    }
}
