//! Skill business logic, with the live Skills API rules and messages.

use std::sync::Arc;

use bytes::Bytes;
use shared::{ApiError, AuthContext, Pagination, Timestamp};

use crate::bundle::{parse_bundle, UploadFile};
use crate::store::{SkillVersionsStore, SkillsStore, StoreError};
use crate::types::{Skill, SkillId, SkillSource, SkillTag, SkillVersion, SkillVersionId, SkillVersionTag};

mod versions;

/// Skills service.
pub struct SkillsService {
    store: Arc<dyn SkillsStore>,
    versions: Arc<dyn SkillVersionsStore>,
}

impl std::fmt::Debug for SkillsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillsService").finish_non_exhaustive()
    }
}

impl SkillsService {
    /// Build with the skill and version stores.
    pub fn new(store: Arc<dyn SkillsStore>, versions: Arc<dyn SkillVersionsStore>) -> Self {
        Self { store, versions }
    }

    /// `POST /v1/skills`: a custom skill and its first version. The label is
    /// `display_name` when sent, else the frontmatter `name` (live skills/02).
    pub async fn create(
        &self,
        ctx: &AuthContext,
        files: Vec<UploadFile>,
        display_name: Option<String>,
    ) -> Result<Skill, ApiError> {
        if let Some(name) = &display_name {
            validate_display_name(name)?;
        }
        let skill_id = SkillId::generate();
        let (version, archive) = new_version(skill_id.clone(), files)?;
        let skill = Skill {
            tag: SkillTag::Skill,
            id: skill_id,
            created_at: version.created_at,
            updated_at: version.created_at,
            display_name: display_name.unwrap_or_else(|| version.name.clone()),
            source: SkillSource::Custom,
            latest_version: version.version.clone(),
            latest_version_id: Some(version.id.clone()),
        };
        self.store
            .create(&ctx.workspace, skill.clone(), version, archive)
            .await
            .map_err(|e| map_store(e, "", ""))?;
        Ok(skill)
    }

    /// `GET /v1/skills/{id}`. Live (skills/44, 45): an unknown or malformed id
    /// is ``Skill not found: <id>``.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Skill, ApiError> {
        let id = SkillId::parse(raw_id).map_err(|_| map_store(StoreError::SkillNotFound, raw_id, ""))?;
        self.store.get(&ctx.workspace, &id).await.map_err(|e| map_store(e, raw_id, ""))
    }

    /// `GET /v1/skills`. agentd has no Anthropic prebuilt skills, so
    /// `source=anthropic` is an empty list.
    pub async fn list(
        &self,
        ctx: &AuthContext,
        source: Option<SkillSource>,
        page: &Pagination,
    ) -> Result<Vec<Skill>, ApiError> {
        if source == Some(SkillSource::Anthropic) {
            return Ok(Vec::new());
        }
        self.store.list(&ctx.workspace, page).await.map_err(|e| map_store(e, "", ""))
    }

    /// `DELETE /v1/skills/{id}`: the skill and every version (live skills/29).
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<(), ApiError> {
        let skill = self.get(ctx, raw_id).await?;
        self.store.delete_skill(&ctx.workspace, &skill.id).await.map_err(|e| map_store(e, raw_id, ""))
    }
}

pub(super) fn new_version(
    skill_id: SkillId,
    files: Vec<UploadFile>,
) -> Result<(SkillVersion, Bytes), ApiError> {
    let bundle = parse_bundle(files)?;
    let now = Timestamp::now();
    let version = SkillVersion {
        tag: SkillVersionTag::SkillVersion,
        skill_id,
        id: SkillVersionId::generate(),
        version: now.as_datetime().timestamp_micros().max(0).to_string(),
        directory: bundle.directory,
        name: bundle.name,
        description: bundle.description,
        created_at: now,
    };
    Ok((version, bundle.archive))
}

/// Live (skills/10, 11).
fn validate_display_name(name: &str) -> Result<(), ApiError> {
    if name.chars().count() > 255 {
        return Err(ApiError::invalid_request("display_name must be at most 255 characters long"));
    }
    if name.chars().any(char::is_control) {
        return Err(ApiError::invalid_request(
            "display_name cannot contain line breaks or control characters",
        ));
    }
    Ok(())
}

/// Live (skills/26, 44): the not-found messages name the skill, and the
/// version as it was asked for.
pub(super) fn map_store(e: StoreError, raw_id: &str, raw_version: &str) -> ApiError {
    match e {
        StoreError::SkillNotFound => ApiError::not_found(format!("Skill not found: {raw_id}")),
        StoreError::VersionNotFound => {
            ApiError::not_found(format!("Skill version not found: {raw_id} version {raw_version}"))
        }
        // Live (cleanup/10): the wording of the only-version refusal.
        StoreError::OnlyVersion => ApiError::invalid_request(
            "cannot delete a Skill's only version. Delete the Skill, or create another version first",
        ),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
