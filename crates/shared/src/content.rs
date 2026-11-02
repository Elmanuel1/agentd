//! Content blocks — shared by session events, deployment `initial_events`,
//! and outcome descriptions.

use serde::{Deserialize, Serialize};

/// A single content block. Only `text` is used by the managed-agents wire
/// today; the enum is exhaustive so new variants force updates at every
/// match site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text payload.
        text: String,
    },
}

impl ContentBlock {
    /// Convenience constructor.
    pub fn text(t: impl Into<String>) -> Self {
        Self::Text { text: t.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_matches_live_fixture() {
        // Live deployment fixture: {"content":[{"text":"run","type":"user.message"...}]}
        // block form: {"text":"run","type":"text"}
        let b = ContentBlock::text("run");
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "run");
    }

    #[test]
    fn deserializes_from_wire() {
        let b: ContentBlock = serde_json::from_str(r#"{"type":"text","text":"hi"}"#).unwrap();
        assert_eq!(b, ContentBlock::text("hi"));
    }
}
