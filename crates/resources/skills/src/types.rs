//! Skill and skill-version records, and their GA and beta wire shapes. The
//! live API serves the GA shapes unless the request sends `skills-2025-10-02`
//! (live skills/14, 16, 19, 23, 43).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shared::{ApiError, Id, Resource, Timestamp};

/// Marker for `skill_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct SkillResource;
impl Resource for SkillResource {
    const PREFIX: &'static str = "skill";
    const LABEL: &'static str = "skill";
}
/// `Id<SkillResource>` alias.
pub type SkillId = Id<SkillResource>;

/// Marker for `skver_…` ids (the beta shape writes them `skill_version_…`).
#[derive(Debug, Clone, Copy)]
pub struct SkillVersionResource;
impl Resource for SkillVersionResource {
    const PREFIX: &'static str = "skver";
    const LABEL: &'static str = "skill version";
}
/// `Id<SkillVersionResource>` alias.
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

/// Where a skill comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    /// Uploaded to this workspace.
    Custom,
    /// Prebuilt by Anthropic.
    Anthropic,
}

/// A stored skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// Always `skill`.
    #[serde(rename = "type")]
    pub tag: SkillTag,
    /// `skill_…`.
    pub id: SkillId,
    /// Created.
    pub created_at: Timestamp,
    /// Updated (a new version moves it).
    pub updated_at: Timestamp,
    /// The label.
    #[serde(alias = "display_title")]
    pub display_name: String,
    /// Where it comes from.
    pub source: SkillSource,
    /// The latest version's numeric timestamp (the beta `latest_version`).
    pub latest_version: String,
    /// The latest version's id.
    #[serde(default)]
    pub latest_version_id: Option<SkillVersionId>,
}

impl Skill {
    /// The GA or beta wire object.
    pub fn wire(&self, beta: bool) -> Value {
        if beta {
            json!({
                "type": "skill", "id": self.id, "created_at": self.created_at, "updated_at": self.updated_at,
                "display_title": self.display_name, "source": self.source, "latest_version": self.latest_version,
            })
        } else {
            json!({
                "type": "skill", "id": self.id, "created_at": self.created_at, "updated_at": self.updated_at,
                "display_name": self.display_name, "source": {"type": self.source},
                "latest_version_id": self.latest_version_id,
            })
        }
    }
}

/// A stored skill version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillVersion {
    /// Always `skill_version`.
    #[serde(rename = "type")]
    pub tag: SkillVersionTag,
    /// The skill.
    pub skill_id: SkillId,
    /// `skver_…`.
    pub id: SkillVersionId,
    /// Its numeric timestamp (epoch microseconds).
    pub version: String,
    /// The bundle's top-level directory.
    pub directory: String,
    /// The frontmatter `name`.
    pub name: String,
    /// The frontmatter `description`.
    pub description: String,
    /// Created.
    pub created_at: Timestamp,
}

impl SkillVersion {
    /// The GA or beta wire object.
    pub fn wire(&self, beta: bool) -> Value {
        if beta {
            json!({
                "type": "skill_version", "id": beta_version_id(&self.id), "skill_id": self.skill_id,
                "name": self.name, "description": self.description, "created_at": self.created_at,
                "version": self.version, "directory": self.directory,
            })
        } else {
            json!({
                "type": "skill_version", "id": self.id, "skill_id": self.skill_id, "name": self.name,
                "description": self.description, "created_at": self.created_at,
            })
        }
    }
}

/// The beta form of a version id: `skill_version_<body>`.
pub fn beta_version_id(id: &SkillVersionId) -> String {
    id.as_str().replacen("skver_", "skill_version_", 1)
}

/// A version as a path names it: `latest`, its id in either form, or its
/// numeric timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionRef {
    /// The newest version.
    Latest,
    /// By id.
    Id(SkillVersionId),
    /// By numeric timestamp.
    Number(String),
}

impl VersionRef {
    /// Live (skills/27): anything else is ``Invalid version id: '…'``.
    pub fn parse(raw: &str) -> Result<Self, ApiError> {
        let invalid = || ApiError::invalid_request(format!("Invalid version id: '{raw}'"));
        if raw == "latest" {
            return Ok(Self::Latest);
        }
        if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
            return Ok(Self::Number(raw.to_owned()));
        }
        let ga = match raw.strip_prefix("skill_version_") {
            Some(body) => format!("skver_{body}"),
            None => raw.to_owned(),
        };
        SkillVersionId::parse(&ga).map(Self::Id).map_err(|_| invalid())
    }
}

/// A skill list page: GA `{data, next_page}`, beta adds `has_more`.
#[derive(Debug, Clone, Serialize)]
pub struct SkillListPage {
    /// The page.
    pub data: Vec<Value>,
    /// Beta only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    /// The next page's cursor, `null` on the last page.
    pub next_page: Option<String>,
}
