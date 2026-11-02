//! File wire types.

use serde::{Deserialize, Serialize};
use shared::{Id, Resource, Timestamp};

/// Marker for `file_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct FileResource;
impl Resource for FileResource {
    const PREFIX: &'static str = "file";
}
/// `Id<FileResource>` alias.
pub type FileId = Id<FileResource>;

/// Constant `"type":"file"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTag {
    /// Always `file`.
    File,
}

/// File metadata object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileMetadata {
    /// Created.
    pub created_at: Timestamp,
    /// Whether content can be downloaded: `false` for uploads (live), which
    /// only tool outputs are not.
    pub downloadable: bool,
    /// When the file expires, `null` without `expires_in_seconds`. The beta
    /// shape (`files-api-2025-04-14`) leaves it out.
    #[serde(default)]
    pub expires_at: Option<Timestamp>,
    /// Original filename.
    pub filename: String,
    /// `file_…`.
    pub id: FileId,
    /// MIME type.
    pub mime_type: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Always `"file"`.
    #[serde(rename = "type")]
    pub tag: FileTag,
    /// The session a file came from (`/mnt/session/outputs/`); left out for
    /// an upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<FileScope>,
}

/// `{"type":"session","id":"sesn_…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileScope {
    /// A session.
    Session {
        /// The session id.
        id: String,
    },
}

impl FileMetadata {
    /// The wire object: the beta shape has no `expires_at`.
    pub fn wire(&self, beta: bool) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if beta {
            if let Some(o) = v.as_object_mut() {
                o.remove("expires_at");
            }
        }
        v
    }

    /// Whether the file has expired at `now`.
    pub fn expired(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|t| t.as_datetime() <= now.as_datetime())
    }
}
