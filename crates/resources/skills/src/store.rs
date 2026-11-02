use async_trait::async_trait;
use bytes::Bytes;
use shared::{Pagination, WorkspaceId};

use crate::{Skill, SkillId, SkillVersion};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such skill.
    #[error("skill not found")]
    SkillNotFound,
    /// No such version.
    #[error("skill version not found")]
    VersionNotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Persistence port. A skill row + its version rows; the zip bundle bytes live
/// in object storage keyed by `(skill_id, version)`.
#[async_trait]
pub trait SkillsStore: Send + Sync {
    /// Create a skill with its first version + stored bundle.
    async fn create(
        &self,
        ws: &WorkspaceId,
        skill: Skill,
        version: SkillVersion,
        bundle: Bytes,
    ) -> Result<(), StoreError>;
    /// Fetch a skill.
    async fn get(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError>;
    /// One page of skills, newest first (fetched DB-side with `limit + 1`).
    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Skill>, StoreError>;
    /// Delete the skill and all its versions/bundles.
    async fn delete_skill(&self, ws: &WorkspaceId, id: &SkillId) -> Result<(), StoreError>;
}

/// A skill's versions and their bundles. Versions have their own lifecycle:
/// they are listed, retrieved and deleted on their own, after the skill and
/// its first version are created together through [`SkillsStore::create`].
#[async_trait]
pub trait SkillVersionsStore: Send + Sync {
    /// One page of a skill's versions, newest first (fetched DB-side).
    async fn versions(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        page: &Pagination,
    ) -> Result<Vec<SkillVersion>, StoreError>;
    /// Delete one version; returns whether the skill has any versions left.
    async fn delete_version(&self, ws: &WorkspaceId, id: &SkillId, version: &str)
        -> Result<bool, StoreError>;
}
