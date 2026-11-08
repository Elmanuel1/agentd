//! The request contract every route shares: the API version, the betas a
//! request opts into, and the betas each resource gates its routes behind.

/// Managed Agents: agents, environments, sessions, vaults, deployments.
pub const MANAGED_AGENTS: &str = "managed-agents-2026-04-01";
/// Memory stores, memories and memory versions.
pub const AGENT_MEMORY: &str = "agent-memory-2026-07-22";
/// The beta Files shapes.
pub const FILES_API: &str = "files-api-2025-04-14";
/// The beta Skills shapes.
pub const SKILLS: &str = "skills-2025-10-02";

/// Every beta name the official SDK (`@anthropic-ai/sdk` 1.3.0, `AnthropicBeta`)
/// knows. Live rejects any other name with a 400.
pub const KNOWN: &[&str] = &[
    "message-batches-2024-09-24",
    "prompt-caching-2024-07-31",
    "computer-use-2024-10-22",
    "computer-use-2025-01-24",
    "pdfs-2024-09-25",
    "token-counting-2024-11-01",
    "token-efficient-tools-2025-02-19",
    "output-128k-2025-02-19",
    FILES_API,
    "mcp-client-2025-04-04",
    "mcp-client-2025-11-20",
    "dev-full-thinking-2025-05-14",
    "interleaved-thinking-2025-05-14",
    "code-execution-2025-05-22",
    "extended-cache-ttl-2025-04-11",
    "context-1m-2025-08-07",
    "context-management-2025-06-27",
    "model-context-window-exceeded-2025-08-26",
    SKILLS,
    "fast-mode-2026-02-01",
    "output-300k-2026-03-24",
    "user-profiles-2026-03-24",
    "user-profiles-2026-08-18",
    "user-profiles-2026-09-04",
    "advisor-tool-2026-03-01",
    MANAGED_AGENTS,
    "cache-diagnosis-2026-04-07",
    "dreaming-2026-04-21",
    "thinking-token-count-2026-05-13",
    "server-side-fallback-2026-06-01",
    "server-side-fallback-2026-07-01",
    "fallback-credit-2026-06-01",
    "fallback-credit-2026-07-01",
    AGENT_MEMORY,
    "mid-conversation-tool-changes-2026-07-01",
    "compact-2026-01-12",
    "computer-use-2025-11-24",
    "mcp-tunnels-2026-06-22",
    "structured-outputs-2025-11-13",
    "task-budgets-2026-03-13",
    "thinking-display-updates-2026-08-18",
    "ce-user-management-2026-07-13",
    "mid-conversation-output-config-2026-07-01",
    "thinking-binding-controls-2026-08-01",
    "mid-conversation-system-clear-at-2026-08-21",
    "compact-2026-09-04",
    "inline-tools-2026-09-15",
    "mcp-client-2026-09-15",
    "ce-plugins-2026-09-01",
    "spend-limit-reads-2026-09-26",
    "telemetry-destinations-2026-08-11",
    "webhooks-2026-10-07",
    "message-threads-2026-08-12",
    "analytics-usage-cost-2026-08-24",
    "experimental-claude-tag-2026-10-21",
];

/// The `anthropic-version` a request was sent with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiVersion(pub String);

/// The betas a request opted into, from every `anthropic-beta` header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BetaSet(Vec<String>);

impl BetaSet {
    /// Parse every header value (comma-separated names). The names not in
    /// [`KNOWN`] are the error.
    pub fn parse<'a>(values: impl IntoIterator<Item = &'a str>) -> Result<Self, Vec<String>> {
        let names: Vec<String> = values
            .into_iter()
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect();
        let unknown: Vec<String> = names.iter().filter(|n| !KNOWN.contains(&n.as_str())).cloned().collect();
        if unknown.is_empty() {
            Ok(Self(names))
        } else {
            Err(unknown)
        }
    }

    /// Whether the request opted into `beta`.
    #[must_use]
    pub fn has(&self, beta: &str) -> bool {
        self.0.iter().any(|n| n == beta)
    }
}

/// Routes a resource serves under a beta. Without the beta, a bodied request
/// to them is read against the GA schema, which knows only `ga_fields`: live
/// answers the first other field with `Failed to parse request body: unknown
/// field "X"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BetaGate {
    /// The beta that opens the routes.
    pub beta: &'static str,
    /// The path prefixes it gates.
    pub prefixes: &'static [&'static str],
    /// The body fields the GA schema accepts.
    pub ga_fields: &'static [&'static str],
}

impl BetaGate {
    /// The Managed Agents gate over `prefixes`.
    #[must_use]
    pub const fn managed(prefixes: &'static [&'static str]) -> Self {
        Self { beta: MANAGED_AGENTS, prefixes, ga_fields: &["name", "description", "metadata"] }
    }

    /// Whether the gate covers `path`.
    #[must_use]
    pub fn covers(&self, path: &str) -> bool {
        self.prefixes.iter().any(|p| path == *p || path.starts_with(&format!("{p}/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_header_and_name_is_read_and_unknown_names_are_named() {
        let set = BetaSet::parse(["managed-agents-2026-04-01, files-api-2025-04-14", AGENT_MEMORY]).unwrap();
        assert!(set.has(FILES_API) && set.has(AGENT_MEMORY) && set.has(MANAGED_AGENTS));
        assert_eq!(BetaSet::parse(["not-a-beta-2020-01-01"]).unwrap_err(), ["not-a-beta-2020-01-01"]);
    }

    #[test]
    fn a_gate_covers_its_prefix_and_children_only() {
        let gate = BetaGate::managed(&["/v1/agents"]);
        assert!(gate.covers("/v1/agents") && gate.covers("/v1/agents/agent_1"));
        assert!(!gate.covers("/v1/agentsx"));
    }
}
