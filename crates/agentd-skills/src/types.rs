use agentd_core::{Id, Page, Pagination, Resource, Timestamp};
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
    /// Build from rows the store fetched DB-side with `limit + 1`
    /// ([`Pagination::fetch_limit`]/[`Pagination::sql_offset`]). Reuses core's
    /// `page_…` cursor; `has_more` mirrors cursor presence.
    #[must_use]
    pub fn from_fetched(rows: Vec<T>, page: &Pagination) -> Self {
        let inner = Page::from_fetched(rows, page);
        Self { data: inner.data, has_more: inner.next_page.is_some(), next_page: inner.next_page }
    }
}
