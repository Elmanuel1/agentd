//! The keys a workspace's archive revokes, a port the app wires to the
//! api-keys store.

use async_trait::async_trait;
use shared::{ApiError, OrgId, Timestamp};

/// Revokes a workspace's keys.
#[async_trait]
pub trait WorkspaceKeys: Send + Sync {
    /// Revoke every active key of `workspace` in `org` at `at`.
    async fn revoke_all(&self, org: &OrgId, workspace: &str, at: Timestamp) -> Result<(), ApiError>;
}
