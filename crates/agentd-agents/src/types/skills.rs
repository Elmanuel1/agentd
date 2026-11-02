use serde::{Deserialize, Serialize};

/// A skill reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillRef {
    /// Anthropic prebuilt skill (`xlsx`, `pdf`, …).
    Anthropic {
        /// Skill id.
        skill_id: String,
    },
    /// Custom skill from the Skills API.
    Custom {
        /// `skill_…` id.
        skill_id: String,
        /// Version (`"latest"` or a number-as-string).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
}

impl SkillRef {
    /// The skill id (dup-checked at validation).
    pub fn skill_id(&self) -> &str {
        match self {
            Self::Anthropic { skill_id } | Self::Custom { skill_id, .. } => skill_id,
        }
    }
}
