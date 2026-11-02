//! A skill upload: a `files[]` part per file (path-qualified filenames), or one
//! zip. Validation messages are the live ones (skills/06-12).

use std::io::{Read, Write};

use bytes::Bytes;
use shared::ApiError;

/// A parsed upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    /// The top-level directory (the frontmatter `name` for a root upload).
    pub directory: String,
    /// The frontmatter `name`.
    pub name: String,
    /// The frontmatter `description`.
    pub description: String,
    /// The files, re-packed as one zip for storage.
    pub archive: Bytes,
}

/// One uploaded file: its path and bytes.
pub type UploadFile = (String, Bytes);

const FRONTMATTER: &str =
    "`SKILL.md` must be UTF-8 text opening with a `---`-fenced YAML frontmatter mapping";

/// Parse the uploaded parts. A single `.zip` part is expanded first.
pub fn parse_bundle(mut files: Vec<UploadFile>) -> Result<Bundle, ApiError> {
    if files.is_empty() {
        return Err(ApiError::invalid_request("files[]: Field required"));
    }
    if files.len() == 1 && files.first().is_some_and(|(p, _)| p.ends_with(".zip")) {
        files = unzip(&files.remove(0).1)?;
    }
    let skill_md = |path: &str| path == "SKILL.md" || path.split('/').nth(1) == Some("SKILL.md");
    let (md_path, md) = files
        .iter()
        .find(|(p, _)| skill_md(p) && p.matches('/').count() <= 1)
        .ok_or_else(|| ApiError::invalid_request("SKILL.md not found in uploaded files"))?;
    let at_root = md_path == "SKILL.md";
    let top = |p: &str| p.split('/').next().unwrap_or_default().to_owned();
    if !at_root {
        let dir = top(md_path);
        if files.iter().any(|(p, _)| !p.contains('/') || top(p) != dir) {
            return Err(ApiError::invalid_request(
                "files must share a single top-level directory, or place `SKILL.md` at the upload root",
            ));
        }
    }
    let text = std::str::from_utf8(md).map_err(|_| ApiError::invalid_request(FRONTMATTER))?;
    let (name, description) = frontmatter(text).ok_or_else(|| ApiError::invalid_request(FRONTMATTER))?;
    let directory = if at_root { name.clone() } else { top(md_path) };
    Ok(Bundle { directory, name, description, archive: zip(&files)? })
}

/// `name` and `description` from a `---`-fenced frontmatter mapping.
fn frontmatter(md: &str) -> Option<(String, String)> {
    let rest = md.trim_start().strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let (mut name, mut description) = (None, None);
    for line in rest.get(..end)?.lines() {
        let value = |v: &str| v.trim().trim_matches(['"', '\'']).to_owned();
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(value(v));
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(value(v));
        }
    }
    Some((name.filter(|n| !n.is_empty())?, description.filter(|d| !d.is_empty())?))
}

fn unzip(bytes: &[u8]) -> Result<Vec<UploadFile>, ApiError> {
    let bad = || ApiError::invalid_request("SKILL.md not found in uploaded files");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| bad())?;
    let mut files = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|_| bad())?;
        if entry.is_dir() {
            continue;
        }
        let mut body = Vec::new();
        entry.read_to_end(&mut body).map_err(|_| bad())?;
        files.push((entry.name().to_owned(), Bytes::from(body)));
    }
    Ok(files)
}

fn zip(files: &[UploadFile]) -> Result<Bytes, ApiError> {
    let fail = |e: &dyn std::fmt::Display| ApiError::internal(format!("skill bundle: {e}"));
    let mut buf = Vec::new();
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
    for (path, body) in files {
        w.start_file(path.as_str(), zip::write::SimpleFileOptions::default()).map_err(|e| fail(&e))?;
        w.write_all(body).map_err(|e| fail(&e))?;
    }
    w.finish().map_err(|e| fail(&e))?;
    Ok(Bytes::from(buf))
}

#[cfg(test)]
mod tests;
