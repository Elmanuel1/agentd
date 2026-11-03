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
    let mut seen = std::collections::HashSet::new();
    if !files.iter().all(|(p, _)| relative(p) && seen.insert(p.as_str())) {
        return Err(ApiError::invalid_request(
            "files[]: paths must be unique and relative, with no empty, `.` or `..` segments",
        ));
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

/// A path inside the bundle: relative, with no empty, `.` or `..` segment,
/// backslash or NUL, so the stored `directory` is always a real folder name.
fn relative(path: &str) -> bool {
    !path.contains(['\\', '\0']) && path.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// An archive expands to at most what an upload may carry (the Skills
/// body limit), in at most this many files.
const MAX_EXPANDED: u64 = 30 * 1024 * 1024;
const MAX_ENTRIES: usize = 1000;

fn unzip(bytes: &[u8]) -> Result<Vec<UploadFile>, ApiError> {
    let bad = || ApiError::invalid_request("SKILL.md not found in uploaded files");
    let too_large =
        || ApiError::invalid_request("files[]: the zip expands past the 30 MB Skills upload limit");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| bad())?;
    if archive.len() > MAX_ENTRIES {
        return Err(too_large());
    }
    let (mut files, mut total) = (Vec::new(), 0u64);
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|_| bad())?;
        if entry.is_dir() {
            continue;
        }
        // The declared size can lie; the read itself stops at what is left.
        let mut body = Vec::new();
        (&mut entry).take(MAX_EXPANDED - total + 1).read_to_end(&mut body).map_err(|_| bad())?;
        total += body.len() as u64;
        if total > MAX_EXPANDED {
            return Err(too_large());
        }
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

/// A stored bundle's files, as a sandbox unpacks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    /// The bundle's top-level directory.
    pub directory: String,
    /// `(path below the directory, bytes)`.
    pub files: Vec<(String, Vec<u8>)>,
}

/// Unzip a stored bundle.
pub fn unpack(archive: &[u8]) -> Result<Unpacked, ApiError> {
    let bad = |e: zip::result::ZipError| ApiError::internal(format!("skill bundle: {e}"));
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(bad)?;
    let mut directory = String::new();
    let mut files = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(bad)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        let Some((top, rel)) = name.split_once('/') else { continue };
        directory = top.to_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| ApiError::internal(format!("skill bundle: {e}")))?;
        files.push((rel.to_owned(), bytes));
    }
    Ok(Unpacked { directory, files })
}

#[cfg(test)]
mod tests;
