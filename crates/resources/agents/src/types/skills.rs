use serde::{Deserialize, Serialize};

/// A skill on an agent, with its pinned version (`latest` unless the request
/// named one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillRef {
    /// Anthropic prebuilt skill (`xlsx`, `pdf`, …).
    Anthropic {
        /// Skill id.
        skill_id: String,
        /// Pinned version.
        #[serde(default = "latest")]
        version: String,
    },
    /// Custom skill from the Skills API.
    Custom {
        /// `skill_…` id.
        skill_id: String,
        /// Pinned version.
        #[serde(default = "latest")]
        version: String,
    },
}

fn latest() -> String {
    "latest".to_owned()
}

impl SkillRef {
    /// The skill id (dup-checked at validation).
    pub fn skill_id(&self) -> &str {
        match self {
            Self::Anthropic { skill_id, .. } | Self::Custom { skill_id, .. } => skill_id,
        }
    }
}

/// A skill as a request sends it: `version` may be left out or `null`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SkillParams {
    /// Anthropic prebuilt skill.
    Anthropic {
        /// Skill id.
        skill_id: String,
        /// Version, `latest` by default.
        #[serde(default)]
        version: Option<String>,
    },
    /// Custom skill.
    Custom {
        /// `skill_…` id.
        skill_id: String,
        /// Version, `latest` by default.
        #[serde(default)]
        version: Option<String>,
    },
}

impl SkillParams {
    /// The stored form, with the default version resolved.
    pub fn resolve(self) -> SkillRef {
        match self {
            Self::Anthropic { skill_id, version } => {
                SkillRef::Anthropic { skill_id, version: version.unwrap_or_else(latest) }
            }
            Self::Custom { skill_id, version } => {
                SkillRef::Custom { skill_id, version: version.unwrap_or_else(latest) }
            }
        }
    }
}
