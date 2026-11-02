use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Pagination, Timestamp};
use bytes::Bytes;

use crate::{
    parse_bundle, Skill, SkillId, SkillResource, SkillSource, SkillTag, SkillVersion, SkillVersionId,
    SkillVersionTag,
};
use crate::{SkillsStore, StoreError};

/// Skills service.
pub struct SkillsService {
    store: Arc<dyn SkillsStore>,
}

impl std::fmt::Debug for SkillsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillsService").finish_non_exhaustive()
    }
}

impl SkillsService {
    /// Build with a store.
    pub fn new(store: Arc<dyn SkillsStore>) -> Self {
        Self { store }
    }

    /// `POST /v1/skills` — create a custom skill from a zip bundle. An explicit
    /// `display_title` overrides the frontmatter-derived title.
    pub async fn create(
        &self,
        ctx: &AuthContext,
        zip_bytes: Bytes,
        display_title: Option<String>,
    ) -> Result<Skill, ApiError> {
        let bundle = parse_bundle(&zip_bytes)?;
        let now = Timestamp::now();
        let version_num = now.as_datetime().timestamp_micros().max(0).to_string();
        let skill_id = SkillId::generate();
        let skill = Skill {
            tag: SkillTag::Skill,
            id: skill_id.clone(),
            created_at: now,
            updated_at: now,
            display_title: display_title.unwrap_or_else(|| bundle.name.clone()),
            source: SkillSource::Custom,
            latest_version: version_num.clone(),
        };
        let version = SkillVersion {
            tag: SkillVersionTag::SkillVersion,
            skill_id,
            id: SkillVersionId::generate(),
            version: version_num,
            directory: bundle.directory,
            name: bundle.name,
            description: bundle.description,
            created_at: now,
        };
        self.store.create(&ctx.workspace, skill.clone(), version, zip_bytes).await.map_err(map_store)?;
        Ok(skill)
    }

    /// `GET /v1/skills/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Skill, ApiError> {
        let id = agentd_core::parse_id::<SkillResource>(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/skills`.
    pub async fn list(&self, ctx: &AuthContext, page: &Pagination) -> Result<Vec<Skill>, ApiError> {
        self.store.list(&ctx.workspace, page).await.map_err(map_store)
    }

    /// `GET /v1/skills/{id}/versions`.
    pub async fn versions(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        page: &Pagination,
    ) -> Result<Vec<SkillVersion>, ApiError> {
        let id = agentd_core::parse_id::<SkillResource>(raw_id)?;
        self.store.versions(&ctx.workspace, &id, page).await.map_err(map_store)
    }

    /// `DELETE /v1/skills/{id}` — remove the skill and all its versions.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<(), ApiError> {
        let id = agentd_core::parse_id::<SkillResource>(raw_id)?;
        self.store.delete_skill(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `DELETE /v1/skills/{id}/versions/{version}` — deleting the last version
    /// removes the skill (matches live).
    pub async fn delete_version(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        version: &str,
    ) -> Result<(), ApiError> {
        let id = agentd_core::parse_id::<SkillResource>(raw_id)?;
        let any_left = self.store.delete_version(&ctx.workspace, &id, version).await.map_err(map_store)?;
        if !any_left {
            self.store.delete_skill(&ctx.workspace, &id).await.map_err(map_store)?;
        }
        Ok(())
    }
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::SkillNotFound => ApiError::not_found("skill not found"),
        StoreError::VersionNotFound => ApiError::not_found("skill version not found"),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
