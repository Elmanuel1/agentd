//! Credential material sealed for the secret store.
//!
//! [`CredentialAuthInput::seal`] is the deliberate inverse of its
//! `Deserialize`: the FULL material, secrets included, as JSON for a
//! [`shared::ports::secrets::SecretStore`]. It is NOT the wire projection and
//! must never reach a client. Every copy of a secret made here is zeroed
//! before it is freed.

use std::fmt::Write as _;

use serde_json::{json, Value};
use shared::ports::secrets::{ExposeSecret, SecretString as Sealed};
use zeroize::Zeroize;

use crate::types::{CredentialAuthInput, OauthRefreshInput, TokenEndpointAuthInput};

/// Sealed material that does not unseal. It says only where the JSON broke:
/// a serde message can quote the value it failed on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("sealed credential material is unreadable ({category} error at line {line}, column {column})")]
pub struct UnsealError {
    category: &'static str,
    line: usize,
    column: usize,
}

impl CredentialAuthInput {
    /// The full material, as JSON this type deserializes from.
    pub fn seal(&self) -> Sealed {
        let mut value = match self {
            Self::McpOauth { mcp_server_url, access_token, expires_at, refresh } => json!({
                "type": "mcp_oauth",
                "mcp_server_url": mcp_server_url,
                "access_token": access_token.expose(),
                "expires_at": expires_at,
                "refresh": refresh.as_ref().map(OauthRefreshInput::seal_value),
            }),
            Self::StaticBearer { mcp_server_url, token } => json!({
                "type": "static_bearer",
                "mcp_server_url": mcp_server_url,
                "token": token.expose(),
            }),
            Self::EnvironmentVariable { secret_name, secret_value, networking, injection_location } => {
                json!({
                    "type": "environment_variable",
                    "secret_name": secret_name,
                    "secret_value": secret_value.expose(),
                    "networking": networking,
                    "injection_location": injection_location,
                })
            }
        };
        let sealed = to_sealed(&value);
        scrub(&mut value);
        sealed
    }

    /// Recover material produced by [`Self::seal`].
    pub fn unseal(sealed: &Sealed) -> Result<Self, UnsealError> {
        serde_json::from_str(sealed.expose_secret()).map_err(|e| UnsealError {
            category: match e.classify() {
                serde_json::error::Category::Io => "io",
                serde_json::error::Category::Syntax => "syntax",
                serde_json::error::Category::Data => "data",
                serde_json::error::Category::Eof => "eof",
            },
            line: e.line(),
            column: e.column(),
        })
    }
}

impl OauthRefreshInput {
    fn seal_value(&self) -> Value {
        let token_endpoint_auth = self.token_endpoint_auth.as_ref().map(|a| match a {
            TokenEndpointAuthInput::None => json!({"type": "none"}),
            TokenEndpointAuthInput::ClientSecretBasic { client_secret } => json!({
                "type": "client_secret_basic",
                "client_secret": client_secret.expose(),
            }),
            TokenEndpointAuthInput::ClientSecretPost { client_secret } => json!({
                "type": "client_secret_post",
                "client_secret": client_secret.expose(),
            }),
        });
        json!({
            "token_endpoint": self.token_endpoint,
            "client_id": self.client_id,
            "scope": self.scope,
            "refresh_token": self.refresh_token.expose(),
            "token_endpoint_auth": token_endpoint_auth,
        })
    }
}

/// Counts what a formatter writes.
struct Len(usize);

impl std::fmt::Write for Len {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0 += s.len();
        Ok(())
    }
}

/// `value` as JSON, written into a buffer sized up front: a growing buffer
/// frees its earlier copies of the secret without zeroing them.
fn to_sealed(value: &Value) -> Sealed {
    let mut len = Len(0);
    // Neither writer can fail, and a `Value` always formats.
    let _ = write!(len, "{value}");
    let mut json = String::with_capacity(len.0);
    let _ = write!(json, "{value}");
    let sealed = Sealed::from(json.as_str());
    json.zeroize();
    sealed
}

/// Zero every string in `value`, the secrets among them.
fn scrub(value: &mut Value) {
    match value {
        Value::String(s) => s.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::Object(fields) => fields.values_mut().for_each(scrub),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OAUTH: &str = r#"{"type":"mcp_oauth","mcp_server_url":"https://mcp.slack.com/mcp",
        "access_token":"xoxp-secret","expires_at":"2026-04-15T00:00:00.000000Z",
        "refresh":{"token_endpoint":"https://slack.com/api/oauth.v2.access","client_id":"123.456",
        "refresh_token":"xoxe-1-secret","scope":"channels:read",
        "token_endpoint_auth":{"type":"client_secret_basic","client_secret":"abc\"quoted"}}}"#;

    #[test]
    fn seal_round_trips_every_variant() {
        for raw in [
            OAUTH,
            r#"{"type":"static_bearer","mcp_server_url":"https://a/mcp","token":"t-1"}"#,
            r#"{"type":"environment_variable","secret_name":"K","secret_value":"v\n2",
                "networking":{"type":"limited","allowed_hosts":["*.x.com"]},
                "injection_location":{"body":false,"header":true}}"#,
        ] {
            let input: CredentialAuthInput = serde_json::from_str(raw).unwrap();
            assert_eq!(CredentialAuthInput::unseal(&input.seal()).unwrap(), input);
        }
    }

    #[test]
    fn sealed_material_is_redacted_in_debug() {
        let input: CredentialAuthInput = serde_json::from_str(OAUTH).unwrap();
        let sealed = input.seal();
        assert!(sealed.expose_secret().contains("xoxe-1-secret"));
        for shown in [format!("{sealed:?}"), format!("{input:?}")] {
            assert!(!shown.contains("xoxp-secret") && !shown.contains("xoxe-1-secret"), "{shown}");
        }
    }

    #[test]
    fn an_unseal_error_never_quotes_the_material() {
        let bad = Sealed::from(r#"{"type":"static_bearer","mcp_server_url":"u","token":12345678}"#);
        let err = CredentialAuthInput::unseal(&bad).unwrap_err().to_string();
        assert!(err.starts_with("sealed credential material is unreadable (data error"), "{err}");
        assert!(!err.contains("12345678"));
        let bad = Sealed::from(r#"{"type":"no_such_kind","token":"hunter2"}"#);
        let err = CredentialAuthInput::unseal(&bad).unwrap_err().to_string();
        assert!(!err.contains("no_such_kind") && !err.contains("hunter2"), "{err}");
    }
}
