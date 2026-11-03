use serde::{Deserialize, Serialize};

/// Model execution speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSpeed {
    /// Default.
    Standard,
    /// Fast mode.
    Fast,
}

/// Reasoning effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    /// `low`.
    Low,
    /// `medium`.
    Medium,
    /// `high`.
    High,
    /// `xhigh`.
    Xhigh,
    /// `max`.
    Max,
}

/// Effort as the wire carries it: `{"type":"high"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effort {
    /// The level.
    #[serde(rename = "type")]
    pub level: EffortLevel,
}

/// Canonical model object: `{"id":"…","speed":"standard"}`, plus `effort` and
/// `inference_geo` when set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Model id.
    pub id: String,
    /// Speed tier.
    pub speed: ModelSpeed,
    /// Reasoning effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Inference geography.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference_geo: Option<String>,
}

impl ModelSpec {
    /// `id` at standard speed.
    pub fn standard(id: impl Into<String>) -> Self {
        Self { id: id.into(), speed: ModelSpeed::Standard, effort: None, inference_geo: None }
    }
}

/// Effort as a request may send it: `"high"` or `{"type":"high"}`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum EffortInput {
    /// `"high"`.
    Level(EffortLevel),
    /// `{"type":"high"}`.
    Object(Effort),
}

/// Accepts the wire's model shorthand: bare string or full object.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ModelInput {
    /// `"claude-sonnet-4-6"`.
    Shorthand(String),
    /// `{"id":"…","speed":"…","effort":…,"inference_geo":…}`.
    Full {
        /// Model id.
        id: String,
        /// Speed (defaults to standard).
        #[serde(default)]
        speed: Option<ModelSpeed>,
        /// Effort.
        #[serde(default)]
        effort: Option<EffortInput>,
        /// Inference geography.
        #[serde(default)]
        inference_geo: Option<String>,
    },
}

impl ModelInput {
    /// Canonicalize to the full object (live rule: shorthand → standard speed).
    pub fn canonicalize(self) -> ModelSpec {
        match self {
            Self::Shorthand(id) => ModelSpec::standard(id),
            Self::Full { id, speed, effort, inference_geo } => ModelSpec {
                id,
                speed: speed.unwrap_or(ModelSpeed::Standard),
                effort: effort.map(|e| match e {
                    EffortInput::Level(level) => Effort { level },
                    EffortInput::Object(e) => e,
                }),
                inference_geo,
            },
        }
    }
}
