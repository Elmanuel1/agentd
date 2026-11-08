//! Content blocks — shared by session events, deployment `initial_events`,
//! and outcome descriptions.

use serde::{Deserialize, Serialize};

/// A single content block, as the SDK sends them: `text`, `image`,
/// `document`, `search_result` and `redacted`. Sources and citations are
/// kept as sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text.
    Text {
        /// The text payload.
        text: String,
    },
    /// An image from base64 data, a URL or a file.
    Image {
        /// `{type:"base64"|"url"|"file", …}`.
        source: serde_json::Value,
    },
    /// A document from base64 data, plain text, a URL or a file.
    Document {
        /// `{type:"base64"|"text"|"url"|"file", …}`.
        source: serde_json::Value,
        /// Context for the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
        /// Title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    /// A search result a tool returned.
    SearchResult {
        /// Citation settings.
        citations: serde_json::Value,
        /// Text content.
        content: Vec<serde_json::Value>,
        /// Where it came from.
        source: String,
        /// Title.
        title: String,
    },
    /// Content withheld from the model.
    Redacted,
}

impl ContentBlock {
    /// Convenience constructor.
    pub fn text(t: impl Into<String>) -> Self {
        Self::Text { text: t.into() }
    }

    /// The text of a `text` block.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
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
    fn every_sdk_block_type_is_accepted() {
        for raw in [
            r#"{"type":"image","source":{"type":"url","url":"https://x/y.png"}}"#,
            r#"{"type":"document","source":{"type":"text","media_type":"text/plain","data":"d"},"title":"t"}"#,
            r#"{"type":"search_result","citations":{"enabled":true},"content":[{"type":"text","text":"r"}],"source":"s","title":"t"}"#,
            r#"{"type":"redacted"}"#,
        ] {
            let b: ContentBlock = serde_json::from_str(raw).unwrap();
            assert_eq!(
                serde_json::to_value(&b).unwrap(),
                serde_json::from_str::<serde_json::Value>(raw).unwrap()
            );
        }
    }

    #[test]
    fn deserializes_from_wire() {
        let b: ContentBlock = serde_json::from_str(r#"{"type":"text","text":"hi"}"#).unwrap();
        assert_eq!(b, ContentBlock::text("hi"));
    }
}
