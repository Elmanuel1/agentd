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
    CreateCredentialRequest, Credential, CredentialAuthInput, CredentialId, CredentialTag,
    UpdateCredentialRequest,
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
            return Err(ApiError::conflict_invalid_request("cannot add credentials to an archived vault"));
        }
        let display_name = required("display_name", req.display_name)?;
        let auth = required("auth", req.auth)?;
        validate_auth(&auth)?;
        let existing =
            self.credentials.list_credentials(&ctx.workspace, &vault_id).await.map_err(map_store)?;
        validate_array_max("credentials", existing.len() + 1, values::CREDENTIALS_PER_VAULT_MAX)?;
        // Unique key among ACTIVE credentials (archived ones freed theirs).
        let key = auth.unique_key().to_owned();
        if existing.iter().any(|c| {
            c.wire.archived_at.is_none() && c.auth.as_ref().map(|a| a.unique_key()) == Some(key.as_str())
        }) {
            return Err(ApiError::conflict_invalid_request(format!(
                "a credential for `{key}` already exists in this vault"
            )));
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
            tag: CredentialTag::Credential,
            updated_at: now,
            vault_id: vault_id.as_str().to_owned(),
        };
        self.credentials
            .insert_credential(
                &ctx.workspace,
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

    /// `POST /v1/vaults/{vid}/credentials/{cid}` — rotation. The variant and
    /// unique key are immutable.
    pub async fn update_credential(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        raw_cred_id: &str,
        req: UpdateCredentialRequest,
    ) -> Result<Credential, ApiError> {
        let (vid, cid) = parse_pair(raw_vault_id, raw_cred_id)?;
        let mut stored =
            self.credentials.get_credential(&ctx.workspace, &vid, &cid).await.map_err(map_store)?;
        if stored.wire.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot update an archived credential"));
        }
        if let Some(new_auth) = req.auth {
            validate_auth(&new_auth)?;
            let old_key = stored.auth.as_ref().map(|a| a.unique_key().to_owned()).unwrap_or_default();
            if new_auth.unique_key() != old_key {
                return Err(ApiError::conflict_invalid_request(format!(
                    "credential keys are immutable: cannot change `{old_key}` to `{}`; archive and create a new credential",
                    new_auth.unique_key()
                )));
            }
            let same_variant = matches!(
                (&stored.auth, &new_auth),
                (Some(CredentialAuthInput::McpOauth { .. }), CredentialAuthInput::McpOauth { .. })
                    | (
                        Some(CredentialAuthInput::StaticBearer { .. }),
                        CredentialAuthInput::StaticBearer { .. }
                    )
                    | (
                        Some(CredentialAuthInput::EnvironmentVariable { .. }),
                        CredentialAuthInput::EnvironmentVariable { .. }
                    )
            );
            if !same_variant {
                return Err(ApiError::conflict_invalid_request(
                    "credential type is immutable; archive and create a new credential",
                ));
            }
            stored.wire.auth = new_auth.to_wire();
            stored.auth = Some(new_auth);
        }
        if let Some(n) = req.display_name {
            stored.wire.display_name = n;
        }
        stored.wire.updated_at = Timestamp::now();
        self.credentials.replace_credential(&ctx.workspace, &vid, stored.clone()).await.map_err(map_store)?;
        Ok(stored.wire)
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
                .replace_credential(&ctx.workspace, &vid, stored.clone())
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
            .delete_credential(&ctx.workspace, &vid, &cid)
            .await
            .map(|c| c.wire)
            .map_err(map_store)
    }
}

/// Live: env-var credentials REQUIRE `auth.networking` — a missing one is
/// `auth.networking: Field required` (an exact nested field-path message serde
/// can't produce, so it's validated here).
fn validate_auth(auth: &CredentialAuthInput) -> Result<(), ApiError> {
    if let CredentialAuthInput::EnvironmentVariable { networking: None, .. } = auth {
        return Err(ApiError::invalid_request("auth.networking: Field required"));
    }
    Ok(())
}
