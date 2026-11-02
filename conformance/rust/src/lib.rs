//! conformance — the swappability oracle.
//!
//! Compares agentd's wire behavior against the live Anthropic API using
//! **structural equality**:
//! - deterministic control-plane responses (CRUD shapes, error envelopes,
//!   status codes, method tables) must match the captured fixtures after
//!   normalizing volatile fields (ids, timestamps, request ids),
//! - generative content is NEVER byte-compared (that's the Outcome/Dream
//!   comparators' job) — only schema + event-type sequences.
//!
//! Two modes:
//! - **fixture mode** (default, hermetic): replay against `fixtures/`,
//! - **live mode** (`CONFORMANCE_LIVE=1` + `ANTHROPIC_API_KEY`): the same
//!   requests hit both agentd and api.anthropic.com and the normalized
//!   bodies are diffed. Live mode creates + cleans real resources — it is
//!   deliberately opt-in.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

use serde_json::Value;

/// Volatile-field normalization: any two conforming servers differ ONLY in
/// these fields, so they are replaced with stable placeholders before
/// comparison.
pub fn normalize(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            Value::Object(map.iter().map(|(k, val)| (k.clone(), normalize_field(k, val))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

fn normalize_field(key: &str, v: &Value) -> Value {
    if let Value::String(s) = v {
        // Resource ids: {prefix}_{20..32 alnum}.
        if let Some((prefix, body)) = s.split_once('_') {
            if !prefix.is_empty()
                && prefix.chars().all(|c| c.is_ascii_lowercase())
                && (20..=32).contains(&body.len())
                && body.bytes().all(|b| b.is_ascii_alphanumeric())
            {
                return Value::String(format!("<{prefix}_id>"));
            }
        }
        // Timestamps.
        if key.ends_with("_at") || key == "scheduled_at" {
            return Value::String("<timestamp>".into());
        }
        if key.ends_with("_runs_at") {
            return Value::String("<timestamp>".into());
        }
        // Hashes.
        if key == "content_sha256" && s.len() == 64 {
            return Value::String("<sha256>".into());
        }
    }
    // upcoming_runs_at arrays of timestamps.
    if key == "upcoming_runs_at" {
        if let Value::Array(items) = v {
            return Value::Array(items.iter().map(|_| Value::String("<timestamp>".into())).collect());
        }
    }
    normalize(v)
}

/// Structural diff after normalization; returns human-readable mismatches.
pub fn diff(expected: &Value, actual: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk("$", &normalize(expected), &normalize(actual), &mut out);
    out
}

fn walk(path: &str, e: &Value, a: &Value, out: &mut Vec<String>) {
    match (e, a) {
        (Value::Object(em), Value::Object(am)) => {
            for (k, ev) in em {
                match am.get(k) {
                    Some(av) => walk(&format!("{path}.{k}"), ev, av, out),
                    None => out.push(format!("{path}.{k}: missing (expected {ev})")),
                }
            }
            for k in am.keys() {
                if !em.contains_key(k) {
                    out.push(format!("{path}.{k}: unexpected extra field"));
                }
            }
        }
        (Value::Array(ea), Value::Array(aa)) => {
            if ea.len() != aa.len() {
                out.push(format!("{path}: array length {} != {}", ea.len(), aa.len()));
            }
            for (i, (ev, av)) in ea.iter().zip(aa.iter()).enumerate() {
                walk(&format!("{path}[{i}]"), ev, av, out);
            }
        }
        _ => {
            if e != a {
                out.push(format!("{path}: expected {e}, got {a}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ids_timestamps_hashes() {
        let v = serde_json::json!({
            "id": "agent_01XdReZgC7eHpg9b9Nvrwdcm",
            "created_at": "2026-07-03T22:32:35.588293Z",
            "content_sha256": "60329bc85480d3c6a566d6101ce5f9fbee836b89e1952cf16326fb392cb59c55",
            "name": "stable"
        });
        let n = normalize(&v);
        assert_eq!(n["id"], "<agent_id>");
        assert_eq!(n["created_at"], "<timestamp>");
        assert_eq!(n["content_sha256"], "<sha256>");
        assert_eq!(n["name"], "stable");
    }

    #[test]
    fn diff_reports_shape_mismatches_only() {
        let a = serde_json::json!({"id":"agent_01XdReZgC7eHpg9b9Nvrwdcm","version":1});
        let b = serde_json::json!({"id":"agent_01SW8QNzJHFPdZf76tukfWPu","version":1});
        assert!(diff(&a, &b).is_empty(), "different ids normalize equal");
        let c = serde_json::json!({"id":"agent_01SW8QNzJHFPdZf76tukfWPu","version":2});
        let d = diff(&a, &c);
        assert_eq!(d.len(), 1);
        assert!(d[0].contains("version"));
    }

    #[test]
    fn diff_catches_missing_and_extra_fields() {
        let expected = serde_json::json!({"a":1,"b":2});
        let actual = serde_json::json!({"a":1,"c":3});
        let d = diff(&expected, &actual);
        assert!(d.iter().any(|m| m.contains(".b: missing")));
        assert!(d.iter().any(|m| m.contains(".c: unexpected")));
    }
}
