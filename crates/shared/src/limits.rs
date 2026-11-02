//! Validation limits and their **exact** wire error strings.
//!
//! Every message template here was captured verbatim from the live API by
//! violating the limit and recording the 400 body. Conformance depends on
//! these strings byte-for-byte — do not "improve" the wording.

use crate::error::ApiError;

/// Numeric limits (live-verified unless noted).
pub mod values {
    /// `agent.name` length range.
    pub const NAME_MIN: usize = 1;
    /// `agent.name` max.
    pub const NAME_MAX: usize = 256;
    /// `agent.description` max.
    pub const DESCRIPTION_MAX: usize = 2048;
    /// `agent.system` max.
    pub const SYSTEM_MAX: usize = 100_000;
    /// `agent.tools` max entries.
    pub const TOOLS_MAX: usize = 128;
    /// `agent.mcp_servers` max entries.
    pub const MCP_SERVERS_MAX: usize = 20;
    /// `agent.skills` max entries (docs; duplicate skill_id verified live).
    pub const SKILLS_MAX: usize = 20;
    /// Agent `metadata` max entries (live-verified; **sessions do not
    /// enforce a count limit** — probe: 9 keys accepted).
    pub const METADATA_MAX_ENTRIES: usize = 16;
    /// Metadata key max length.
    pub const METADATA_KEY_MAX: usize = 64;
    /// Metadata value max length.
    pub const METADATA_VALUE_MAX: usize = 512;
    /// Memory content max bytes (live: `value length must be at most 102400 bytes`).
    pub const MEMORY_CONTENT_MAX_BYTES: usize = 102_400;
    /// Outcome `max_iterations` range (live-verified).
    pub const OUTCOME_MAX_ITERATIONS_MIN: u32 = 1;
    /// Outcome `max_iterations` upper bound.
    pub const OUTCOME_MAX_ITERATIONS_MAX: u32 = 20;
    /// sha256 hex length for preconditions.
    pub const SHA256_HEX_LEN: usize = 64;
    /// Deployments per organization (docs).
    pub const DEPLOYMENTS_PER_ORG_MAX: usize = 1000;
    /// Credentials per vault (docs).
    pub const CREDENTIALS_PER_VAULT_MAX: usize = 20;
    /// Memory stores per session (docs).
    pub const MEMORY_STORES_PER_SESSION_MAX: usize = 8;
    /// File resources per session (docs).
    pub const FILE_RESOURCES_PER_SESSION_MAX: usize = 999;
}

/// `name: maximum string length is 256` / `name: minimum string length is 1`
pub fn validate_string_len(field: &str, value: &str, min: usize, max: usize) -> Result<(), ApiError> {
    let n = value.chars().count();
    if n < min {
        return Err(ApiError::invalid_request(format!("{field}: minimum string length is {min}")));
    }
    if n > max {
        return Err(ApiError::invalid_request(format!("{field}: maximum string length is {max}")));
    }
    Ok(())
}

/// `tools: value must contain no more than 128 item(s)`
pub fn validate_array_max(field: &str, len: usize, max: usize) -> Result<(), ApiError> {
    if len > max {
        return Err(ApiError::invalid_request(format!(
            "{field}: value must contain no more than {max} item(s)"
        )));
    }
    Ok(())
}

/// `content: value length must be at most 102400 bytes`
pub fn validate_bytes_max(field: &str, len: usize, max: usize) -> Result<(), ApiError> {
    if len > max {
        return Err(ApiError::invalid_request(format!("{field}: value length must be at most {max} bytes")));
    }
    Ok(())
}

/// `…: value must be greater than or equal to 1 and less than or equal to 20`
pub fn validate_range_u32(field_path: &str, value: u32, min: u32, max: u32) -> Result<(), ApiError> {
    if value < min || value > max {
        return Err(ApiError::invalid_request(format!(
            "{field_path}: value must be greater than or equal to {min} and less than or equal to {max}"
        )));
    }
    Ok(())
}

/// `model: Field required`
pub fn required<T>(field: &str, value: Option<T>) -> Result<T, ApiError> {
    value.ok_or_else(|| ApiError::invalid_request(format!("{field}: Field required")))
}

/// `` `model.id`: model "gpt-4": model is not supported ``
pub fn validate_model_supported<'a>(model_id: &'a str, catalog: &ModelCatalog) -> Result<&'a str, ApiError> {
    if catalog.supports(model_id) {
        Ok(model_id)
    } else {
        Err(ApiError::invalid_request(format!("`model.id`: model \"{model_id}\": model is not supported")))
    }
}

/// `precondition.content_sha256: value length must be 64 characters (and 1 more validation errors)`
///
/// Live behavior: a wrong-length value fails BOTH the length check and the
/// 64-hex pattern check, so the message always carries the
/// `(and 1 more validation errors)` suffix (verified with `"deadbeef"`).
pub fn validate_sha256_hex(field_path: &str, value: &str) -> Result<(), ApiError> {
    let len_ok = value.len() == values::SHA256_HEX_LEN;
    let hex_ok = value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    match (len_ok, hex_ok) {
        (true, true) => Ok(()),
        (false, _) => Err(ApiError::invalid_request(format!(
            "{field_path}: value length must be {} characters (and 1 more validation errors)",
            values::SHA256_HEX_LEN
        ))),
        (true, false) => {
            Err(ApiError::invalid_request(format!("{field_path}: value does not match the required pattern")))
        }
    }
}

/// The set of model IDs this deployment will accept.
#[derive(Debug, Clone)]
pub struct ModelCatalog {
    models: Vec<String>,
}

impl ModelCatalog {
    /// Build from an explicit list.
    #[must_use]
    pub fn new(models: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self { models: models.into_iter().map(Into::into).collect() }
    }

    /// Default catalog mirroring currently-served Anthropic model aliases.
    #[must_use]
    pub fn default_catalog() -> Self {
        Self::new([
            "claude-fable-5",
            "claude-sonnet-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-opus-4-5",
            "claude-sonnet-4-6",
            "claude-sonnet-4-5",
            "claude-haiku-4-5",
        ])
    }

    /// Whether a model id is accepted.
    #[must_use]
    pub fn supports(&self, id: &str) -> bool {
        self.models.iter().any(|m| m == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(r: Result<(), ApiError>) -> String {
        r.unwrap_err().message
    }

    #[test]
    fn string_limits_exact_live_strings() {
        assert_eq!(
            msg(validate_string_len("name", &"a".repeat(257), 1, 256)),
            "name: maximum string length is 256"
        );
        assert_eq!(msg(validate_string_len("name", "", 1, 256)), "name: minimum string length is 1");
        assert_eq!(
            msg(validate_string_len("description", &"d".repeat(2049), 0, 2048)),
            "description: maximum string length is 2048"
        );
        assert_eq!(
            msg(validate_string_len("system", &"s".repeat(100_001), 0, 100_000)),
            "system: maximum string length is 100000"
        );
        assert!(validate_string_len("name", "ok", 1, 256).is_ok());
    }

    #[test]
    fn array_limits_exact_live_strings() {
        assert_eq!(
            msg(validate_array_max("tools", 129, 128)),
            "tools: value must contain no more than 128 item(s)"
        );
        assert_eq!(
            msg(validate_array_max("mcp_servers", 21, 20)),
            "mcp_servers: value must contain no more than 20 item(s)"
        );
        assert!(validate_array_max("tools", 128, 128).is_ok());
    }

    #[test]
    fn memory_bytes_exact_live_string() {
        assert_eq!(
            msg(validate_bytes_max("content", 102_401, 102_400)),
            "content: value length must be at most 102400 bytes"
        );
        assert!(validate_bytes_max("content", 102_400, 102_400).is_ok());
    }

    #[test]
    fn range_exact_live_string() {
        assert_eq!(
            msg(validate_range_u32(
                "events.0.user_define_outcome.max_iterations",
                21,
                1,
                20
            )),
            "events.0.user_define_outcome.max_iterations: value must be greater than or equal to 1 and less than or equal to 20"
        );
        assert!(validate_range_u32("x", 20, 1, 20).is_ok());
        assert!(validate_range_u32("x", 0, 1, 20).is_err());
    }

    #[test]
    fn required_exact_live_string() {
        let e = required::<()>("model", None).unwrap_err();
        assert_eq!(e.message, "model: Field required");
        assert_eq!(required("model", Some(1)).unwrap(), 1);
    }

    #[test]
    fn model_catalog_exact_live_string() {
        let cat = ModelCatalog::default_catalog();
        let e = validate_model_supported("gpt-4", &cat).unwrap_err();
        assert_eq!(e.message, "`model.id`: model \"gpt-4\": model is not supported");
        assert!(validate_model_supported("claude-sonnet-4-6", &cat).is_ok());
    }

    #[test]
    fn sha256_precondition_exact_live_string() {
        // Live probe: precondition "deadbeef" → 400
        // "precondition.content_sha256: value length must be 64 characters (and 1 more validation errors)"
        let e = validate_sha256_hex("precondition.content_sha256", "deadbeef").unwrap_err();
        assert_eq!(
            e.message,
            "precondition.content_sha256: value length must be 64 characters (and 1 more validation errors)"
        );
        assert!(validate_sha256_hex("p", &"a".repeat(64)).is_ok());
        assert!(validate_sha256_hex("p", &"A".repeat(64)).is_err(), "uppercase hex rejected");
    }
}
