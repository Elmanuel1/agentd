//! Skill versions: create, list, retrieve, download and delete.

use bytes::Bytes;
use shared::{ApiError, AuthContext, Pagination};

use super::{map_store, new_version, SkillsService};
use crate::bundle::UploadFile;
use crate::types::{SkillVersion, SkillVersionId, VersionRef};

impl SkillsService {
    /// `POST /v1/skills/{id}/versions`: the new version becomes the latest.
    pub async fn create_version(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        files: Vec<UploadFile>,
    ) -> Result<SkillVersion, ApiError> {
        let skill = self.get(ctx, raw_id).await?;
        let (version, archive) = new_version(skill.id.clone(), files)?;
        self.versions
            .add(&ctx.workspace, version.clone(), archive)
            .await
            .map_err(|e| map_store(e, raw_id, ""))?;
        Ok(version)
    }

    /// `GET /v1/skills/{id}/versions`: newest first, or oldest first for the
    /// beta (live skills/21, skills/23).
    pub async fn versions(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        page: &Pagination,
        oldest_first: bool,
    ) -> Result<Vec<SkillVersion>, ApiError> {
        let skill = self.get(ctx, raw_id).await?;
        self.versions
            .versions(&ctx.workspace, &skill.id, page, oldest_first)
            .await
            .map_err(|e| map_store(e, raw_id, ""))
    }

    /// `GET /v1/skills/{id}/versions/{version}` (`latest`, an id or a number).
    pub async fn version(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        raw_version: &str,
    ) -> Result<SkillVersion, ApiError> {
        let which = VersionRef::parse(raw_version)?;
        let skill = self.get(ctx, raw_id).await?;
        self.versions
            .get(&ctx.workspace, &skill.id, &which)
            .await
            .map_err(|e| map_store(e, raw_id, raw_version))
    }

    /// `GET /v1/skills/{id}/versions/{version}/content`: the version's zip.
    /// Live (skills/28) names the version by its numeric timestamp here.
    pub async fn version_bundle(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        raw_version: &str,
    ) -> Result<Bytes, ApiError> {
        if let VersionRef::Id(_) = VersionRef::parse(raw_version)? {
            return Err(ApiError::invalid_request(format!(
                "Invalid version format: '{raw_version}'. Version must be a numeric timestamp (e.g., '1759178010641129')."
            )));
        }
        let version = self.version(ctx, raw_id, raw_version).await?;
        self.versions.bundle(&ctx.workspace, &version).await.map_err(|e| map_store(e, raw_id, raw_version))
    }

    /// `DELETE /v1/skills/{id}/versions/{version}`. The only version cannot be
    /// deleted (delete the skill instead); deleting the latest moves the
    /// skill's latest to the newest one left, in the store's transaction.
    pub async fn delete_version(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        raw_version: &str,
    ) -> Result<SkillVersionId, ApiError> {
        let which = VersionRef::parse(raw_version)?;
        let skill = self.get(ctx, raw_id).await?;
        let version = self
            .versions
            .get(&ctx.workspace, &skill.id, &which)
            .await
            .map_err(|e| map_store(e, raw_id, raw_version))?;
        self.versions
            .delete_version(&ctx.workspace, &version)
            .await
            .map_err(|e| map_store(e, raw_id, raw_version))?;
        Ok(version.id)
    }
}
