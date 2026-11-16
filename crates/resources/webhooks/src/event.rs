//! A delivery's body, as live sends it: `{"type": "event", "id": "whe_<32
//! hex>", "created_at", "data": {"type", "id", "organization_id",
//! "workspace_id"}}`, with `vault_id` on a credential's events.

use rand::RngCore;
use serde::Serialize;
use shared::Timestamp;

/// What a delivery reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// A session's status became `running`.
    SessionRunStarted,
    /// A session's status became `idle`.
    SessionIdled,
    /// A session's status became `terminated`.
    SessionTerminated,
    /// A vault was created.
    VaultCreated,
    /// A vault was archived.
    VaultArchived,
    /// A vault was deleted.
    VaultDeleted,
    /// A credential of the vault was created.
    CredentialCreated {
        /// Its vault.
        vault_id: String,
    },
    /// A credential of the vault was archived.
    CredentialArchived {
        /// Its vault.
        vault_id: String,
    },
    /// A credential of the vault was deleted.
    CredentialDeleted {
        /// Its vault.
        vault_id: String,
    },
    /// An `mcp_oauth` credential's refresh failed (H-5).
    CredentialRefreshFailed {
        /// Its vault.
        vault_id: String,
    },
}

impl EventKind {
    /// Its `data.type`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SessionRunStarted => "session.status_run_started",
            Self::SessionIdled => "session.status_idled",
            Self::SessionTerminated => "session.status_terminated",
            Self::VaultCreated => "vault.created",
            Self::VaultArchived => "vault.archived",
            Self::VaultDeleted => "vault.deleted",
            Self::CredentialCreated { .. } => "vault_credential.created",
            Self::CredentialArchived { .. } => "vault_credential.archived",
            Self::CredentialDeleted { .. } => "vault_credential.deleted",
            Self::CredentialRefreshFailed { .. } => "vault_credential.refresh_failed",
        }
    }

    fn vault_id(&self) -> Option<&str> {
        match self {
            Self::CredentialCreated { vault_id }
            | Self::CredentialArchived { vault_id }
            | Self::CredentialDeleted { vault_id }
            | Self::CredentialRefreshFailed { vault_id } => Some(vault_id),
            _ => None,
        }
    }
}

/// One delivery: an event to one endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// The event's `whe_` id, sent as `webhook-id` on every attempt.
    pub webhook_id: String,
    /// The endpoint's key.
    pub endpoint_key: String,
    /// Where it goes.
    pub endpoint_url: String,
    /// The exact bytes every attempt signs and sends.
    pub body: String,
}

#[derive(Serialize)]
struct Body<'a> {
    #[serde(rename = "type")]
    tag: &'static str,
    id: &'a str,
    created_at: String,
    data: Data<'a>,
}

#[derive(Serialize)]
struct Data<'a> {
    #[serde(rename = "type")]
    tag: &'static str,
    id: &'a str,
    organization_id: &'a str,
    workspace_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    vault_id: Option<&'a str>,
}

/// A new event's id and body.
pub(super) fn body(org: &str, workspace: &str, kind: &EventKind, id: &str) -> (String, String) {
    let mut raw = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut raw);
    let webhook_id = format!("whe_{}", hex::encode(raw));
    let body = Body {
        tag: "event",
        id: &webhook_id,
        created_at: Timestamp::now().to_wire(),
        data: Data {
            tag: kind.name(),
            id,
            organization_id: org,
            workspace_id: workspace,
            vault_id: kind.vault_id(),
        },
    };
    let text = serde_json::to_string(&body).unwrap_or_default();
    (webhook_id, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_has_live_s_shape() {
        let (id, text) =
            body("org_1", "wrkspc_1", &EventKind::CredentialArchived { vault_id: "vlt_1".into() }, "vcrd_1");
        assert!(id.starts_with("whe_") && id.len() == 36, "{id}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!((v["type"].as_str(), v["id"].as_str()), (Some("event"), Some(id.as_str())));
        assert_eq!(
            v["created_at"].as_str().unwrap().len(),
            "2026-11-06T01:23:36.986000Z".len(),
            "six digits"
        );
        assert_eq!(
            v["data"],
            serde_json::json!({"type": "vault_credential.archived", "id": "vcrd_1", "organization_id": "org_1",
                "workspace_id": "wrkspc_1", "vault_id": "vlt_1"})
        );
    }
}
