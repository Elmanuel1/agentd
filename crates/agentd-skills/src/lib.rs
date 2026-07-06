//! agentd-skills — the standard Anthropic **Skills API** (`/v1/skills`).
//!
//! NOTE: this is NOT a managed-agents resource. It's the shared Skills API on
//! its own beta header **`skills-2025-10-02`** (probed live 2026-07-06). A
//! custom skill is a zip bundle with a single top-level folder containing
//! `SKILL.md`; the API extracts the folder name (`directory`) and the
//! `name`/`description` from the SKILL.md YAML frontmatter. Bundles are stored
//! in object storage; only metadata lives in the DB. Agents reference skills by
//! id via the `skills[]` field (see `agentd-agents::SkillRef`).

#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)
)]

pub mod routes;

use std::io::Read;
use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Id, Page, Pagination, Resource, Timestamp, WorkspaceId};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Marker for `skill_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct SkillResource;
impl Resource for SkillResource {
    const PREFIX: &'static str = "skill";
    const LABEL: &'static str = "skill";
}
/// `Id<SkillResource>`.
pub type SkillId = Id<SkillResource>;

/// Marker for `skill_version_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct SkillVersionResource;
impl Resource for SkillVersionResource {
    const PREFIX: &'static str = "skill_version";
    const LABEL: &'static str = "skill version";
}
/// `Id<SkillVersionResource>`.
pub type SkillVersionId = Id<SkillVersionResource>;

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

/// A skill's provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Workspace-authored, uploaded via the Skills API.
    Custom,
    /// Anthropic pre-built (xlsx/pptx/docx/pdf).
    Anthropic,
}

/// A skill — live shape
/// `{type, id, created_at, updated_at, display_title, source, latest_version}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// Always `"skill"`.
    #[serde(rename = "type")]
    pub tag: SkillTag,
    /// `skill_…`.
    pub id: SkillId,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
    /// Display title (from `SKILL.md` name or the `display_title` form field).
    pub display_title: String,
    /// Provenance.
    pub source: SkillSource,
    /// The latest version id (an integer-as-string).
    pub latest_version: String,
}

/// An immutable skill version — live shape `{type, skill_id, id, version,
/// directory, name, description, created_at}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillVersion {
    /// Always `"skill_version"`.
    #[serde(rename = "type")]
    pub tag: SkillVersionTag,
    /// Parent skill.
    pub skill_id: SkillId,
    /// `skill_version_…`.
    pub id: SkillVersionId,
    /// Integer-as-string version.
    pub version: String,
    /// Top-level folder name in the bundle.
    pub directory: String,
    /// From the SKILL.md frontmatter `name`.
    pub name: String,
    /// From the SKILL.md frontmatter `description`.
    pub description: String,
    /// Created.
    pub created_at: Timestamp,
}

/// The Skills API list envelope — live shape `{data, has_more, next_page}` (a
/// third pagination shape carrying BOTH `has_more` and the `next_page` cursor).
#[derive(Debug, Clone, Serialize)]
pub struct SkillListPage<T> {
    /// Items on this page.
    pub data: Vec<T>,
    /// Whether more results follow.
    pub has_more: bool,
    /// Opaque cursor for the next page; omitted on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<String>,
}

impl<T> SkillListPage<T> {
    /// Slice an ordered set to the requested page. Reuses core's `page_…`
    /// cursor; `has_more` mirrors cursor presence.
    #[must_use]
    pub fn paginate(all: Vec<T>, page: &Pagination) -> Self {
        let inner = Page::paginate(all, page);
        Self {
            data: inner.data,
            has_more: inner.next_page.is_some(),
            next_page: inner.next_page,
        }
    }
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

/// The parsed contents of an uploaded bundle (folder + frontmatter fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    /// Top-level folder name.
    pub directory: String,
    /// `name` from SKILL.md frontmatter.
    pub name: String,
    /// `description` from SKILL.md frontmatter.
    pub description: String,
}

/// Parse an uploaded skill zip: require a single top-level folder containing
/// `SKILL.md`, and read `name`/`description` from its YAML frontmatter. Matches
/// the live 400 when the layout is wrong.
pub fn parse_bundle(zip_bytes: &[u8]) -> Result<Bundle, ApiError> {
    let layout_err = || {
        ApiError::invalid_request(
            "Zip must contain a top-level folder with all files inside it, including SKILL.md",
        )
    };
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|_| layout_err())?;

    // Every entry must live under one shared top-level folder.
    let mut top: Option<String> = None;
    let mut skill_md_body: Option<String> = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|_| layout_err())?;
        let name = entry.name().to_owned();
        let first = name.split('/').next().unwrap_or_default().to_owned();
        if first.is_empty() {
            return Err(layout_err());
        }
        match &top {
            Some(t) if t != &first => return Err(layout_err()),
            _ => top = Some(first.clone()),
        }
        if name == format!("{first}/SKILL.md") {
            let mut body = String::new();
            entry.read_to_string(&mut body).map_err(|_| layout_err())?;
            skill_md_body = Some(body);
        }
    }
    let directory = top.ok_or_else(layout_err)?;
    let body = skill_md_body.ok_or_else(layout_err)?;
    let (name, description) = parse_frontmatter(&body);
    Ok(Bundle {
        name: if name.is_empty() { directory.clone() } else { name },
        directory,
        description,
    })
}

/// Read `name`/`description` from a `---`-delimited YAML frontmatter block.
fn parse_frontmatter(md: &str) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    let trimmed = md.trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (name, description);
    };
    let Some(end) = rest.find("\n---") else {
        return (name, description);
    };
    for line in rest[..end].lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().trim_matches(['"', '\'']).to_owned();
        } else if let Some(v) = line.strip_prefix("description:") {
            description = v.trim().trim_matches(['"', '\'']).to_owned();
        }
    }
    (name, description)
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
    /// All skills, newest first.
    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Skill>, StoreError>;
    /// A skill's versions, newest first.
    async fn versions(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
    ) -> Result<Vec<SkillVersion>, StoreError>;
    /// Delete one version; returns whether the skill has any versions left.
    async fn delete_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: &str,
    ) -> Result<bool, StoreError>;
    /// Delete the skill and all its versions/bundles.
    async fn delete_skill(&self, ws: &WorkspaceId, id: &SkillId) -> Result<(), StoreError>;
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
        self.store
            .create(&ctx.workspace, skill.clone(), version, zip_bytes)
            .await
            .map_err(map_store)?;
        Ok(skill)
    }

    /// `GET /v1/skills/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Skill, ApiError> {
        let id = agentd_core::parse_id::<SkillResource>(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/skills`.
    pub async fn list(&self, ctx: &AuthContext) -> Result<Vec<Skill>, ApiError> {
        self.store.list(&ctx.workspace).await.map_err(map_store)
    }

    /// `GET /v1/skills/{id}/versions`.
    pub async fn versions(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<Vec<SkillVersion>, ApiError> {
        let id = agentd_core::parse_id::<SkillResource>(raw_id)?;
        self.store
            .versions(&ctx.workspace, &id)
            .await
            .map_err(map_store)
    }

    /// `DELETE /v1/skills/{id}` — remove the skill and all its versions.
    pub async fn delete(&self, ctx: &AuthContext, raw_id: &str) -> Result<(), ApiError> {
        let id = agentd_core::parse_id::<SkillResource>(raw_id)?;
        self.store
            .delete_skill(&ctx.workspace, &id)
            .await
            .map_err(map_store)
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
        let any_left = self
            .store
            .delete_version(&ctx.workspace, &id, version)
            .await
            .map_err(map_store)?;
        if !any_left {
            self.store
                .delete_skill(&ctx.workspace, &id)
                .await
                .map_err(map_store)?;
        }
        Ok(())
    }
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
    use std::io::Write;

    fn zip_with(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            for (name, body) in entries {
                w.start_file(*name, opts).unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    #[test]
    fn parses_top_level_folder_and_frontmatter() {
        let zip = zip_with(&[(
            "my-skill/SKILL.md",
            "---\nname: My Skill\ndescription: does a thing\n---\n# body\n",
        )]);
        let b = parse_bundle(&zip).unwrap();
        assert_eq!(b.directory, "my-skill");
        assert_eq!(b.name, "My Skill");
        assert_eq!(b.description, "does a thing");
    }

    #[test]
    fn rejects_root_level_skill_md() {
        let zip = zip_with(&[("SKILL.md", "# no folder")]);
        let err = parse_bundle(&zip).unwrap_err();
        assert!(err.message.starts_with("Zip must contain a top-level folder"));
    }

    #[test]
    fn rejects_multiple_top_level_folders() {
        let zip = zip_with(&[("a/SKILL.md", "x"), ("b/other.txt", "y")]);
        assert!(parse_bundle(&zip).is_err());
    }
}
