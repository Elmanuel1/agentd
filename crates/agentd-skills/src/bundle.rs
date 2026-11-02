use std::io::Read;

use agentd_core::ApiError;

/// The parsed contents of an uploaded bundle (folder + frontmatter fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    /// Top-level folder name.
    pub directory: String,
    /// `name` from SKILL.md frontmatter.
    pub name: String,
    /// `description` from SKILL.md frontmatter.
    pub description: String,
}

/// Parse an uploaded skill zip: require a single top-level folder containing
/// `SKILL.md`, and read `name`/`description` from its YAML frontmatter. Matches
/// the live 400 when the layout is wrong.
pub fn parse_bundle(zip_bytes: &[u8]) -> Result<Bundle, ApiError> {
    let layout_err = || {
        ApiError::invalid_request(
            "Zip must contain a top-level folder with all files inside it, including SKILL.md",
        )
    };
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|_| layout_err())?;

    // Every entry must live under one shared top-level folder.
    let mut top: Option<String> = None;
    let mut skill_md_body: Option<String> = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|_| layout_err())?;
        let name = entry.name().to_owned();
        let first = name.split('/').next().unwrap_or_default().to_owned();
        if first.is_empty() {
            return Err(layout_err());
        }
        match &top {
            Some(t) if t != &first => return Err(layout_err()),
            _ => top = Some(first.clone()),
        }
        if name == format!("{first}/SKILL.md") {
            let mut body = String::new();
            entry.read_to_string(&mut body).map_err(|_| layout_err())?;
            skill_md_body = Some(body);
        }
    }
    let directory = top.ok_or_else(layout_err)?;
    let body = skill_md_body.ok_or_else(layout_err)?;
    let (name, description) = parse_frontmatter(&body);
    Ok(Bundle { name: if name.is_empty() { directory.clone() } else { name }, directory, description })
}

/// Read `name`/`description` from a `---`-delimited YAML frontmatter block.
fn parse_frontmatter(md: &str) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    let trimmed = md.trim_start();
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (name, description);
    };
    let Some(end) = rest.find("\n---") else {
        return (name, description);
    };
    for line in rest[..end].lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = v.trim().trim_matches(['"', '\'']).to_owned();
        } else if let Some(v) = line.strip_prefix("description:") {
            description = v.trim().trim_matches(['"', '\'']).to_owned();
        }
    }
    (name, description)
}

#[cfg(test)]
mod tests;
