//! agentd-skills — the Skills support resource (`/v1/skills`).
//!
//! Skills + skill versions, delete-only lifecycle. A skill version carries
//! the SKILL.md content (agents load it by `skill_id` + `version`).
//! Wire shapes follow the documented Skills API (flagged: unprobed live).

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod routes;

use std::collections::BTreeMap;
use std::sync::Arc;

use agentd_core::limits::required;
use agentd_core::{ApiError, AuthContext, Id, Resource, Timestamp, WorkspaceId};
use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Marker for `skill_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct SkillResource;
impl Resource for SkillResource {
    const PREFIX: &'static str = "skill";
}
/// `Id<SkillResource>` alias.
pub type SkillId = Id<SkillResource>;

/// Constant `"type":"skill"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillTag {
    /// Always `skill`.
    Skill,
}

/// Constant `"type":"skill_version"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillVersionTag {
    /// Always `skill_version`.
    SkillVersion,
}

/// A skill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skill {
    /// Created.
    pub created_at: Timestamp,
    /// Description.
    pub description: String,
    /// Display name.
    pub display_name: String,
    /// `skill_…`.
    pub id: SkillId,
    /// Latest version number (null before any version exists).
    pub latest_version: Option<u64>,
    /// Always `"skill"`.
    #[serde(rename = "type")]
    pub tag: SkillTag,
    /// Updated.
    pub updated_at: Timestamp,
}

/// One immutable skill version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillVersion {
    /// SKILL.md content.
    pub content: String,
    /// Created.
    pub created_at: Timestamp,
    /// Parent skill.
    pub skill_id: SkillId,
    /// Always `"skill_version"`.
    #[serde(rename = "type")]
    pub tag: SkillVersionTag,
    /// Version number (1-based).
    pub version: u64,
}

/// `POST /v1/skills` body.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateSkillRequest {
    /// Required.
    pub display_name: Option<String>,
    /// Optional.
    pub description: Option<String>,
}

/// `POST /v1/skills/{id}/versions` body.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateVersionRequest {
    /// Required — the SKILL.md content.
    pub content: Option<String>,
}

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
    Backend(String),
}

/// Persistence port.
#[async_trait]
pub trait SkillsStore: Send + Sync {
    /// Insert a skill.
    async fn insert(&self, ws: &WorkspaceId, s: Skill) -> Result<(), StoreError>;
    /// Fetch a skill.
    async fn get(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError>;
    /// Replace a skill.
    async fn replace(&self, ws: &WorkspaceId, s: Skill) -> Result<(), StoreError>;
    /// Delete a skill (and versions).
    async fn delete(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError>;
    /// List skills.
    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Skill>, StoreError>;
    /// Append a version.
    async fn push_version(&self, ws: &WorkspaceId, v: SkillVersion) -> Result<(), StoreError>;
    /// Fetch a version.
    async fn get_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: u64,
    ) -> Result<SkillVersion, StoreError>;
    /// Delete a version.
    async fn delete_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: u64,
    ) -> Result<SkillVersion, StoreError>;
    /// List versions ascending.
    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
    ) -> Result<Vec<SkillVersion>, StoreError>;
}

type SkillEntry = (Skill, BTreeMap<u64, SkillVersion>);

/// In-memory store.
#[derive(Debug, Default)]
pub struct InMemorySkillsStore {
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<SkillId, SkillEntry>>>,
}

impl InMemorySkillsStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SkillsStore for InMemorySkillsStore {
    async fn insert(&self, ws: &WorkspaceId, s: Skill) -> Result<(), StoreError> {
        self.inner
            .write()
            .entry(ws.clone())
            .or_default()
            .insert(s.id.clone(), (s, BTreeMap::new()));
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError> {
        self.inner
            .read()
            .get(ws)
            .and_then(|m| m.get(id))
            .map(|(s, _)| s.clone())
            .ok_or(StoreError::SkillNotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, s: Skill) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&s.id))
            .ok_or(StoreError::SkillNotFound)?;
        e.0 = s;
        Ok(())
    }

    async fn delete(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError> {
        self.inner
            .write()
            .get_mut(ws)
            .and_then(|m| m.remove(id))
            .map(|(s, _)| s)
            .ok_or(StoreError::SkillNotFound)
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Skill>, StoreError> {
        Ok(self
            .inner
            .read()
            .get(ws)
            .map(|m| m.values().map(|(s, _)| s.clone()).collect())
            .unwrap_or_default())
    }

    async fn push_version(&self, ws: &WorkspaceId, v: SkillVersion) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&v.skill_id))
            .ok_or(StoreError::SkillNotFound)?;
        e.1.insert(v.version, v);
        Ok(())
    }

    async fn get_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: u64,
    ) -> Result<SkillVersion, StoreError> {
        let g = self.inner.read();
        let e = g
            .get(ws)
            .and_then(|m| m.get(id))
            .ok_or(StoreError::SkillNotFound)?;
        e.1.get(&version).cloned().ok_or(StoreError::VersionNotFound)
    }

    async fn delete_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: u64,
    ) -> Result<SkillVersion, StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(id))
            .ok_or(StoreError::SkillNotFound)?;
        e.1.remove(&version).ok_or(StoreError::VersionNotFound)
    }

    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
    ) -> Result<Vec<SkillVersion>, StoreError> {
        let g = self.inner.read();
        let e = g
            .get(ws)
            .and_then(|m| m.get(id))
            .ok_or(StoreError::SkillNotFound)?;
        Ok(e.1.values().cloned().collect())
    }
}

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

    /// `POST /v1/skills`.
    pub async fn create(
        &self,
        ctx: &AuthContext,
        req: CreateSkillRequest,
    ) -> Result<Skill, ApiError> {
        let display_name = required("display_name", req.display_name)?;
        let now = Timestamp::now();
        let s = Skill {
            created_at: now,
            description: req.description.unwrap_or_default(),
            display_name,
            id: SkillId::generate(),
            latest_version: None,
            tag: SkillTag::Skill,
            updated_at: now,
        };
        self.store
            .insert(&ctx.workspace, s.clone())
            .await
            .map_err(map_store)?;
        Ok(s)
    }

    /// `GET /v1/skills/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Skill, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/skills`.
    pub async fn list(&self, ctx: &AuthContext) -> Result<Vec<Skill>, ApiError> {
        self.store.list(&ctx.workspace).await.map_err(map_store)
    }

    /// `DELETE /v1/skills/{id}`.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<Skill, ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .delete(&ctx.workspace, &id)
            .await
            .map_err(map_store)
    }

    /// `POST /v1/skills/{id}/versions` — next integer version.
    pub async fn create_version(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: CreateVersionRequest,
    ) -> Result<SkillVersion, ApiError> {
        let id = parse_id(raw_id)?;
        let mut skill = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        let content = required("content", req.content)?;
        let next = skill.latest_version.unwrap_or(0) + 1;
        let v = SkillVersion {
            content,
            created_at: Timestamp::now(),
            skill_id: id.clone(),
            tag: SkillVersionTag::SkillVersion,
            version: next,
        };
        self.store
            .push_version(&ctx.workspace, v.clone())
            .await
            .map_err(map_store)?;
        skill.latest_version = Some(next);
        skill.updated_at = Timestamp::now();
        self.store
            .replace(&ctx.workspace, skill)
            .await
            .map_err(map_store)?;
        Ok(v)
    }

    /// `GET /v1/skills/{id}/versions/{v}`.
    pub async fn get_version(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        version: u64,
    ) -> Result<SkillVersion, ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .get_version(&ctx.workspace, &id, version)
            .await
            .map_err(map_store)
    }

    /// `GET /v1/skills/{id}/versions`.
    pub async fn list_versions(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<Vec<SkillVersion>, ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .list_versions(&ctx.workspace, &id)
            .await
            .map_err(map_store)
    }

    /// `DELETE /v1/skills/{id}/versions/{v}`.
    pub async fn delete_version(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        version: u64,
    ) -> Result<SkillVersion, ApiError> {
        let id = parse_id(raw_id)?;
        self.store
            .delete_version(&ctx.workspace, &id, version)
            .await
            .map_err(map_store)
    }
}

fn parse_id(raw: &str) -> Result<SkillId, ApiError> {
    SkillId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::SkillNotFound => ApiError::not_found("skill not found"),
        StoreError::VersionNotFound => ApiError::not_found("skill version not found"),
        StoreError::Backend(m) => ApiError::internal(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentd_core::OrgId;

    fn svc() -> SkillsService {
        SkillsService::new(Arc::new(InMemorySkillsStore::new()))
    }

    fn ctx() -> AuthContext {
        AuthContext {
            org: OrgId("org_t".into()),
            workspace: WorkspaceId("wrkspc_01t".into()),
            api_key_id: "apikey_01t".into(),
        }
    }

    #[tokio::test]
    async fn skill_and_version_lifecycle() {
        let s = svc();
        let skill = s
            .create(
                &ctx(),
                CreateSkillRequest {
                    display_name: Some("citing-sources".into()),
                    description: Some("cite everything".into()),
                },
            )
            .await
            .unwrap();
        assert!(skill.id.as_str().starts_with("skill_01"));
        assert_eq!(skill.latest_version, None);

        let v1 = s
            .create_version(
                &ctx(),
                skill.id.as_str(),
                CreateVersionRequest {
                    content: Some("# SKILL v1".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(v1.version, 1);
        let v2 = s
            .create_version(
                &ctx(),
                skill.id.as_str(),
                CreateVersionRequest {
                    content: Some("# SKILL v2".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(v2.version, 2);

        let updated = s.get(&ctx(), skill.id.as_str()).await.unwrap();
        assert_eq!(updated.latest_version, Some(2));

        let versions = s.list_versions(&ctx(), skill.id.as_str()).await.unwrap();
        assert_eq!(versions.len(), 2);

        s.delete_version(&ctx(), skill.id.as_str(), 1).await.unwrap();
        assert_eq!(
            s.get_version(&ctx(), skill.id.as_str(), 1)
                .await
                .unwrap_err()
                .status,
            404
        );

        s.delete(&ctx(), skill.id.as_str()).await.unwrap();
        assert_eq!(s.get(&ctx(), skill.id.as_str()).await.unwrap_err().status, 404);
    }

    #[tokio::test]
    async fn required_fields() {
        let s = svc();
        let e = s.create(&ctx(), CreateSkillRequest::default()).await.unwrap_err();
        assert_eq!(e.message, "display_name: Field required");
    }
}
