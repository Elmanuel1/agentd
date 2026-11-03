use async_trait::async_trait;
use bytes::Bytes;
use shared::{Pagination, WorkspaceId};

use crate::{Skill, SkillId, SkillVersion, VersionRef};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such skill.
    #[error("skill not found")]
    SkillNotFound,
    /// No such version.
    #[error("skill version not found")]
    VersionNotFound,
    /// The version is the skill's only one (delete the skill instead).
    #[error("only version")]
    OnlyVersion,
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
    /// Replace a skill row (a new version moves its latest).
    async fn replace(&self, ws: &WorkspaceId, skill: Skill) -> Result<(), StoreError>;
    /// Delete the skill and all its versions/bundles.
    async fn delete_skill(&self, ws: &WorkspaceId, id: &SkillId) -> Result<(), StoreError>;
}

/// A skill's versions and their bundles. Versions have their own lifecycle:
/// they are added, listed, retrieved and deleted on their own, after the
/// skill and its first version are created together through
/// [`SkillsStore::create`].
#[async_trait]
pub trait SkillVersionsStore: Send + Sync {
    /// One page of a skill's versions, newest first or, with `oldest_first`,
    /// oldest first (fetched DB-side).
    async fn versions(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        page: &Pagination,
        oldest_first: bool,
    ) -> Result<Vec<SkillVersion>, StoreError>;
    /// Add a version and its bundle, and make it the skill's latest unless a
    /// newer version landed first.
    async fn add(&self, ws: &WorkspaceId, version: SkillVersion, bundle: Bytes) -> Result<(), StoreError>;
    /// The version `which` names.
    async fn get(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        which: &VersionRef,
    ) -> Result<SkillVersion, StoreError>;
    /// How many versions the skill has.
    async fn count(&self, ws: &WorkspaceId, id: &SkillId) -> Result<usize, StoreError>;
    /// A version's bundle.
    async fn bundle(&self, ws: &WorkspaceId, version: &SkillVersion) -> Result<Bytes, StoreError>;
    /// Delete one version and its bundle, moving the skill's latest to the
    /// newest left; the only version is [`StoreError::OnlyVersion`].
    async fn delete_version(&self, ws: &WorkspaceId, version: &SkillVersion) -> Result<(), StoreError>;
}
