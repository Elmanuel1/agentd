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
    /// Whether content can be downloaded.
    pub downloadable: bool,
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
}
