//! Placeholders: what a `cloud` sandbox sees in place of each
//! `environment_variable` credential's value. Each is signed with the egress
//! key over the session, workspace, vault and credential ids, so the egress
//! swap can find the credential's secret and refuses one copied into another
//! session.

use serde::{Deserialize, Serialize};
use shared::hash::Signer;

/// Every placeholder starts with this.
pub const PREFIX: &str = "agentd-ph-";

/// What a placeholder stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceholderClaims {
    /// The session whose sandbox holds it.
    pub session: String,
    /// The workspace.
    pub ws: String,
    /// The vault holding the credential.
    pub vault: String,
    /// The credential.
    pub credential: String,
}

/// Signs and checks placeholders.
#[derive(Debug, Clone)]
pub struct Placeholders {
    signer: Signer,
}

impl Placeholders {
    /// Sign with the egress key.
    pub fn new(egress_key: impl Into<Vec<u8>>) -> Self {
        Self { signer: Signer::new(egress_key).derive("placeholder") }
    }

    /// The placeholder for `claims`.
    pub fn sign(&self, claims: &PlaceholderClaims) -> String {
        self.signer.sign(PREFIX, claims)
    }

    /// What a placeholder this key signed stands for.
    pub fn verify(&self, placeholder: &str) -> Option<PlaceholderClaims> {
        self.signer.verify(PREFIX, placeholder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_names_its_credential_and_verifies_only_under_its_key() {
        let claims = PlaceholderClaims {
            session: "sesn_1".into(),
            ws: "wrkspc_1".into(),
            vault: "vlt_1".into(),
            credential: "cred_1".into(),
        };
        let keys = Placeholders::new(b"egress".to_vec());
        let p = keys.sign(&claims);
        assert!(p.starts_with(PREFIX));
        assert_eq!(keys.verify(&p), Some(claims));
        assert_eq!(Placeholders::new(b"other".to_vec()).verify(&p), None);
        assert_eq!(crate::WorkTokens::new(b"egress".to_vec()).verify(&p), None);
    }
}
