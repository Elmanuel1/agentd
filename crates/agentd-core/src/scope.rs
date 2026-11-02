//! Resource scope serialization rule.
//!
//! Live-verified: **only environments** carry `"scope":"organization"` on the
//! wire. Agents, vaults, memory stores, sessions, and deployments are
//! workspace-scoped but the wire omits any scope/workspace field entirely.
//! Entity serializers must therefore only include a scope field on
//! environments — emitting `workspace_id` anywhere would break conformance.

use serde::{Deserialize, Serialize};

/// Wire scope value (currently only ever `organization`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Org-visible resource (environments).
    Organization,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_live_wire_string() {
        assert_eq!(serde_json::to_string(&Scope::Organization).unwrap(), "\"organization\"");
    }
}
