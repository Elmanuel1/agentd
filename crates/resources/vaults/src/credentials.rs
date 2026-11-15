//! Vault credential business logic.
//!
//! Enforced rules (docs + wire conventions):
//! - unique key per vault (`mcp_server_url` / `secret_name`) among **active**
//!   credentials → 409 on duplicate,
//! - ≤20 credentials per vault,
//! - keys immutable: update may rotate secrets/display_name, not the key or
//!   the variant,
//! - archive purges the secret and frees the key.

use shared::limits::{required, validate_array_max, values};
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, Timestamp};

use crate::service::{map_store, parse_pair, parse_vault_id, VaultsService};
use crate::store::StoredCredential;
use crate::types::{
    CreateCredentialRequest, Credential, CredentialAuthInput, CredentialAuthWire, CredentialId, CredentialTag,
};

impl VaultsService {
    /// `POST /v1/vaults/{id}/credentials`.
    pub async fn create_credential(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        req: CreateCredentialRequest,
    ) -> Result<Credential, ApiError> {
        let vault_id = parse_vault_id(raw_vault_id)?;
        let vault = self.vaults.get_vault(&ctx.workspace, &vault_id).await.map_err(map_store)?;
        if vault.archived_at.is_some() {
            return Err(ApiError::invalid_request("Vault is archived."));
        }
        let display_name = required("display_name", req.display_name)?;
        let auth = required("auth", req.auth)?;
        validate_auth(&auth)?;
        let existing =
            self.credentials.list_credentials(&ctx.workspace, &vault_id).await.map_err(map_store)?;
        validate_array_max("credentials", existing.len() + 1, values::CREDENTIALS_PER_VAULT_MAX)?;
        // One active credential per MCP server URL, across MCP types, and per
        // environment variable; an archived credential frees its key (Q10).
        if existing.iter().any(|c| c.wire.archived_at.is_none() && same_key(&c.wire.auth, &auth)) {
            return Err(ApiError::conflict_invalid_request(match auth {
                CredentialAuthInput::EnvironmentVariable { .. } => {
                    "A credential already exists for this environment variable."
                }
                _ => "A credential already exists for this MCP server URL.",
            }));
        }
        let now = Timestamp::now();
        req.metadata.validate(MetadataPolicy::capped())?;
        let cred = Credential {
            archived_at: None,
            auth: auth.to_wire(),
            created_at: now,
            display_name,
            id: CredentialId::generate(),
            metadata: req.metadata,
            tag: CredentialTag::VaultCredential,
            updated_at: now,
            vault_id: vault_id.as_str().to_owned(),
        };
        self.credentials
            .insert_credential(
                &ctx.workspace,
                &ctx.org,
                &vault_id,
                StoredCredential { wire: cred.clone(), auth: Some(auth) },
            )
            .await
            .map_err(map_store)?;
        Ok(cred)
    }

    /// `GET /v1/vaults/{vid}/credentials/{cid}` — metadata only, never secrets.
    pub async fn get_credential(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        raw_cred_id: &str,
    ) -> Result<Credential, ApiError> {
        let (vid, cid) = parse_pair(raw_vault_id, raw_cred_id)?;
        self.credentials.get_credential(&ctx.workspace, &vid, &cid).await.map(|c| c.wire).map_err(map_store)
    }

    /// `GET /v1/vaults/{id}/credentials`.
    pub async fn list_credentials(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
    ) -> Result<Vec<Credential>, ApiError> {
        let vid = parse_vault_id(raw_vault_id)?;
        Ok(self
            .credentials
            .list_credentials(&ctx.workspace, &vid)
            .await
            .map_err(map_store)?
            .into_iter()
            .map(|c| c.wire)
            .collect())
    }

    /// `POST /v1/vaults/{vid}/credentials/{cid}/archive` — purges the secret
    /// and frees the unique key.
    pub async fn archive_credential(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        raw_cred_id: &str,
    ) -> Result<Credential, ApiError> {
        let (vid, cid) = parse_pair(raw_vault_id, raw_cred_id)?;
        let mut stored =
            self.credentials.get_credential(&ctx.workspace, &vid, &cid).await.map_err(map_store)?;
        if stored.wire.archived_at.is_none() {
            stored.wire.archived_at = Some(Timestamp::now());
            stored.wire.updated_at = Timestamp::now();
            stored.auth = None; // secret purged
            self.credentials
                .archive_credential(&ctx.workspace, &ctx.org, &vid, stored.clone())
                .await
                .map_err(map_store)?;
        }
        Ok(stored.wire)
    }

    /// `DELETE /v1/vaults/{vid}/credentials/{cid}`.
    pub async fn delete_credential(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        raw_cred_id: &str,
    ) -> Result<Credential, ApiError> {
        let (vid, cid) = parse_pair(raw_vault_id, raw_cred_id)?;
        self.credentials
            .delete_credential(&ctx.workspace, &ctx.org, &vid, &cid)
            .await
            .map(|c| c.wire)
            .map_err(map_store)
    }
}

/// Live requires an env-var credential's `auth.networking` and a refresh's
/// `client_id`, naming each by its nested path (`auth.networking: Field
/// required`), which serde cannot, so they are checked here.
pub(crate) fn validate_auth(auth: &CredentialAuthInput) -> Result<(), ApiError> {
    match auth {
        CredentialAuthInput::EnvironmentVariable { networking: None, .. } => {
            Err(ApiError::invalid_request("auth.networking: Field required"))
        }
        CredentialAuthInput::McpOauth { refresh: Some(r), .. } if r.client_id.is_none() => {
            Err(ApiError::invalid_request("auth.refresh.client_id: Field required"))
        }
        _ => Ok(()),
    }
}

/// Whether `stored` holds the key `new` would take: the MCP server URL for
/// either MCP type, the variable name for an environment variable.
fn same_key(stored: &CredentialAuthWire, new: &CredentialAuthInput) -> bool {
    match (stored, new) {
        (
            CredentialAuthWire::EnvironmentVariable { secret_name, .. },
            CredentialAuthInput::EnvironmentVariable { .. },
        ) => secret_name == new.unique_key(),
        (CredentialAuthWire::EnvironmentVariable { .. }, _)
        | (_, CredentialAuthInput::EnvironmentVariable { .. }) => false,
        (
            CredentialAuthWire::McpOauth { mcp_server_url, .. }
            | CredentialAuthWire::StaticBearer { mcp_server_url },
            _,
        ) => mcp_server_url == new.unique_key(),
    }
}
