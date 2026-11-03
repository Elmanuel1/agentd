//! Work tokens: what a sandbox's worker authenticates with. A token names
//! one work item (with its session, workspace and organization) and is
//! signed by agentd, so it grants that work's routes and nothing else.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use shared::hash::Signer;

/// Every work token starts with this.
pub const PREFIX: &str = "agentd-wt-";

/// What a token grants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkClaims {
    /// The work item.
    pub work: String,
    /// Its session.
    pub session: String,
    /// The workspace.
    pub ws: String,
    /// The organization.
    pub org: String,
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

    /// A random key, for a single replica.
    pub fn random() -> Self {
        Self::new(rand::random::<[u8; 32]>().to_vec())
    }

    /// The token for `claims`.
    pub fn mint(&self, claims: &WorkClaims) -> String {
        self.signer.sign(PREFIX, claims)
    }

    /// The claims of a token this key signed.
    pub fn verify(&self, token: &str) -> Option<WorkClaims> {
        self.signer.verify(PREFIX, token)
    }

    /// The work `secret`: base64url JSON with `sessions_token`, as the SDK's
    /// environment worker reads it.
    pub fn secret(&self, claims: &WorkClaims) -> String {
        let payload = serde_json::json!({"sessions_token": self.mint(claims)});
        URL_SAFE_NO_PAD.encode(payload.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> WorkClaims {
        WorkClaims {
            work: "work_1".into(),
            session: "sesn_1".into(),
            ws: "wrkspc_1".into(),
            org: "org_1".into(),
        }
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
    fn the_secret_carries_the_sessions_token() {
        let tokens = WorkTokens::new(b"k".to_vec());
        let raw = URL_SAFE_NO_PAD.decode(tokens.secret(&claims())).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(tokens.verify(v["sessions_token"].as_str().unwrap()), Some(claims()));
    }
}
