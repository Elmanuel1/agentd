//! Work tokens: what a session's worker authenticates with. A token names
//! one session (with its workspace and organization) and is signed by
//! agentd, so it reaches that session's work item and session and nothing
//! else. The work secret carries it as `sessions_token`.

use std::collections::BTreeMap;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use shared::hash::Signer;

/// Every work token starts with this.
pub const PREFIX: &str = "agentd-wt-";

/// What a token grants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkClaims {
    /// The session, which is also its work item's id.
    pub session: String,
    /// The workspace.
    pub ws: String,
    /// The organization.
    pub org: String,
}

/// The work secret's payload: live's nine keys, in live's order. agentd runs
/// no Claude Code, so the keys only live's own harness reads are empty.
#[derive(Debug, Serialize)]
struct Secret<'a> {
    version: u32,
    session_ingress_token: &'a str,
    api_base_url: &'a str,
    sources: [(); 0],
    auth: [(); 0],
    sessions_token: &'a str,
    claude_code_args: BTreeMap<String, String>,
    environment_variables: &'a BTreeMap<String, String>,
    use_code_sessions: bool,
}

/// Mints and checks work tokens.
#[derive(Debug, Clone)]
pub struct WorkTokens {
    signer: Signer,
}

impl WorkTokens {
    /// Sign with `key`.
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self { signer: Signer::new(key).derive("work") }
    }

    /// The token for `claims`.
    pub fn mint(&self, claims: &WorkClaims) -> String {
        self.signer.sign(PREFIX, claims)
    }

    /// The claims of a token this key signed.
    pub fn verify(&self, token: &str) -> Option<WorkClaims> {
        self.signer.verify(PREFIX, token)
    }

    /// The work `secret`: base64url JSON the SDK's worker decodes itself,
    /// carrying the item's token and its `environment_variables`.
    pub fn secret(
        &self,
        claims: &WorkClaims,
        api_base_url: &str,
        environment_variables: &BTreeMap<String, String>,
    ) -> String {
        let token = self.mint(claims);
        let payload = Secret {
            version: 1,
            session_ingress_token: "",
            api_base_url,
            sources: [],
            auth: [],
            sessions_token: &token,
            claude_code_args: BTreeMap::new(),
            environment_variables,
            use_code_sessions: true,
        };
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> WorkClaims {
        WorkClaims { session: "sesn_1".into(), ws: "wrkspc_1".into(), org: "org_1".into() }
    }

    #[test]
    fn a_token_verifies_only_under_its_key_and_unchanged() {
        let tokens = WorkTokens::new(b"k".to_vec());
        let t = tokens.mint(&claims());
        assert_eq!(tokens.verify(&t), Some(claims()));
        assert_eq!(WorkTokens::new(b"other".to_vec()).verify(&t), None);
        let tampered = t.replacen("agentd-wt-e", "agentd-wt-f", 1);
        assert_eq!(tokens.verify(&tampered), None);
        assert_eq!(tokens.verify("sk-ant-123"), None);
    }

    #[test]
    fn the_secret_has_live_s_nine_keys_in_live_s_order_and_types() {
        let tokens = WorkTokens::new(b"k".to_vec());
        let vars = BTreeMap::from([("GITHUB_TOKEN".to_owned(), "agentd-ph-x".to_owned())]);
        let raw = URL_SAFE_NO_PAD.decode(tokens.secret(&claims(), "https://agentd.test", &vars)).unwrap();
        let text = String::from_utf8(raw.clone()).unwrap();
        let keys = [
            "version",
            "session_ingress_token",
            "api_base_url",
            "sources",
            "auth",
            "sessions_token",
            "claude_code_args",
            "environment_variables",
            "use_code_sessions",
        ];
        let at: Vec<usize> = keys.iter().map(|k| text.find(&format!("\"{k}\":")).unwrap()).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{text}");
        let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 9);
        assert_eq!(v["version"], 1);
        assert_eq!(v["session_ingress_token"], "");
        assert_eq!(v["api_base_url"], "https://agentd.test");
        assert_eq!((v["sources"].clone(), v["auth"].clone()), (serde_json::json!([]), serde_json::json!([])));
        assert_eq!(v["claude_code_args"], serde_json::json!({}));
        assert_eq!(v["environment_variables"], serde_json::json!({"GITHUB_TOKEN": "agentd-ph-x"}));
        assert_eq!(v["use_code_sessions"], true);
        assert_eq!(tokens.verify(v["sessions_token"].as_str().unwrap()), Some(claims()));
    }
}
