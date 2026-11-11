//! The model-facing definitions of the agent toolset. Names, descriptions and
//! input fields follow the live toolset as captured in Managed Agents event
//! logs (`read` takes `file_path` and `view_range`, `write` takes
//! `file_path` and `content`).

use serde_json::json;

use crate::model::ModelTool;

/// Every tool of `agent_toolset_20260401`, in toolset order.
pub const TOOLSET: [&str; 8] = ["bash", "read", "write", "edit", "glob", "grep", "web_fetch", "web_search"];

/// The tools a grader gets: it reads the work and changes nothing.
pub const READ_ONLY: [&str; 4] = ["bash", "read", "glob", "grep"];

/// The definition of toolset tool `name`; `None` for a name outside it.
pub fn spec(name: &str) -> Option<ModelTool> {
    let (description, schema) = match name {
        "bash" => (
            "Run bash commands in a shell session. The working directory carries over between calls.",
            json!({"type": "object", "properties": {
                "command": {"type": "string", "description": "The command to run."}},
                "required": ["command"]}),
        ),
        "read" => (
            "Read a file from the sandbox filesystem. Lines come back numbered; `view_range` picks a 1-indexed, inclusive range, -1 for the end of the file.",
            json!({"type": "object", "properties": {
                "file_path": {"type": "string", "description": "Absolute path of the file."},
                "view_range": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2}},
                "required": ["file_path"]}),
        ),
        "write" => (
            "Write a file to the sandbox filesystem, creating parent directories and replacing any existing file.",
            json!({"type": "object", "properties": {
                "file_path": {"type": "string", "description": "Absolute path of the file."},
                "content": {"type": "string", "description": "The full contents."}},
                "required": ["file_path", "content"]}),
        ),
        "edit" => (
            "Perform string replacement in a file. `old_string` must match exactly once unless `replace_all` is true.",
            json!({"type": "object", "properties": {
                "file_path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"},
                "replace_all": {"type": "boolean"}},
                "required": ["file_path", "old_string", "new_string"]}),
        ),
        "glob" => (
            "Perform fast file pattern matching using glob patterns (`**/*.json`). Returns matching paths.",
            json!({"type": "object", "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string", "description": "Directory to search; the working directory by default."}},
                "required": ["pattern"]}),
        ),
        "grep" => (
            "Search text using regex patterns (extended regular expressions). Returns `path:line:text` matches.",
            json!({"type": "object", "properties": {
                "pattern": {"type": "string"},
                "path": {"type": "string", "description": "File or directory to search; the working directory by default."},
                "glob": {"type": "string", "description": "Only files whose name matches this glob."},
                "case_insensitive": {"type": "boolean"},
                "files_with_matches": {"type": "boolean", "description": "List matching files instead of lines."}},
                "required": ["pattern"]}),
        ),
        "web_fetch" => (
            "Fetch content from a URL. Returns the response body as text.",
            json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
        ),
        "web_search" => (
            "Search the web for information. Returns titles, URLs and snippets.",
            json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
        ),
        _ => return None,
    };
    Some(ModelTool { name: name.to_owned(), description: description.to_owned(), input_schema: schema })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_toolset_tool_has_a_spec_with_required_fields() {
        for name in TOOLSET {
            let s = spec(name).unwrap();
            assert_eq!(s.name, name);
            assert!(s.input_schema["required"].as_array().is_some_and(|r| !r.is_empty()), "{name}");
        }
        assert!(spec("str_replace_based_edit_tool").is_none());
    }
}
