//! The tokens a Messages API response's `usage` reports.

use serde_json::Value;
use sessions::events::ModelUsage;

/// The call's tokens. A compaction-beta reply also lists `iterations`, and
/// its top level leaves the compaction out, so a count the iterations carry
/// is their sum.
pub(super) fn read(u: &Value) -> ModelUsage {
    let n = |u: &Value, f: &str| crate::json::at(u, f).as_u64().unwrap_or(0);
    let its = crate::json::at(u, "/iterations").as_array().cloned().unwrap_or_default();
    let total = |f: &str| match its.iter().any(|it| it.pointer(f).is_some()) {
        true => its.iter().map(|it| n(it, f)).sum(),
        false => n(u, f),
    };
    ModelUsage {
        cache_creation_input_tokens: total("/cache_creation_input_tokens"),
        cache_read_input_tokens: total("/cache_read_input_tokens"),
        input_tokens: total("/input_tokens"),
        output_tokens: total("/output_tokens"),
    }
}
