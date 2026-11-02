//! Vault/credential business logic.
//!
//! Enforced rules (docs + wire conventions):
//! - unique key per vault (`mcp_server_url` / `secret_name`) among **active**
//!   credentials → 409 on duplicate,
//! - ≤20 credentials per vault,
//! - keys immutable: update may rotate secrets/display_name, not the key or
//!   the variant,
//! - archive purges the secret and frees the key.

use std::sync::Arc;

use agentd_core::limits::{required, validate_array_max, validate_string_len, values};
use agentd_core::metadata::MetadataPolicy;
use agentd_core::{ApiError, AuthContext, Pagination, Timestamp};

use crate::store::{StoreError, StoredCredential, VaultsStore};
use crate::types::{
    CreateCredentialRequest, CreateVaultRequest, Credential, CredentialId, CredentialTag,
    UpdateCredentialRequest, UpdateVaultRequest, Vault, VaultId, VaultTag,
};

/// Vault service.
pub struct VaultsService {
    store: Arc<dyn VaultsStore>,
}

impl std::fmt::Debug for VaultsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultsService").finish_non_exhaustive()
    }
}

impl VaultsService {
    /// Build with a store.
    pub fn new(store: Arc<dyn VaultsStore>) -> Self {
        Self { store }
    }

    // ── Vault CRUD ────────────────────────────────────────────────────────

    /// `POST /v1/vaults`.
    pub async fn create_vault(&self, ctx: &AuthContext, req: CreateVaultRequest) -> Result<Vault, ApiError> {
        let display_name = required("display_name", req.display_name)?;
        validate_string_len("display_name", &display_name, values::NAME_MIN, values::NAME_MAX)?;
        req.metadata.validate(MetadataPolicy::capped())?;
        let now = Timestamp::now();
        let vault = Vault {
            archived_at: None,
            created_at: now,
            display_name,
            id: VaultId::generate(),
            metadata: req.metadata,
            tag: VaultTag::Vault,
            updated_at: now,
        };
        self.store.insert_vault(&ctx.workspace, vault.clone()).await.map_err(map_store)?;
        Ok(vault)
    }

    /// `GET /v1/vaults/{id}`.
    pub async fn get_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        self.store.get_vault(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/vaults`.
    pub async fn list_vaults(&self, ctx: &AuthContext, page: &Pagination) -> Result<Vec<Vault>, ApiError> {
        self.store.list_vaults(&ctx.workspace, page).await.map_err(map_store)
    }

    /// `POST /v1/vaults/{id}`.
    pub async fn update_vault(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: UpdateVaultRequest,
    ) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        let mut vault = self.store.get_vault(&ctx.workspace, &id).await.map_err(map_store)?;
        if vault.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot update an archived vault"));
        }
        if let Some(n) = req.display_name {
            validate_string_len("display_name", &n, values::NAME_MIN, values::NAME_MAX)?;
            vault.display_name = n;
        }
        if let Some(m) = req.metadata {
            m.validate(MetadataPolicy::capped())?;
            vault.metadata = m;
        }
        vault.updated_at = Timestamp::now();
        self.store.replace_vault(&ctx.workspace, vault.clone()).await.map_err(map_store)?;
        Ok(vault)
    }

    /// `POST /v1/vaults/{id}/archive`.
    pub async fn archive_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        let mut vault = self.store.get_vault(&ctx.workspace, &id).await.map_err(map_store)?;
        if vault.archived_at.is_none() {
            vault.archived_at = Some(Timestamp::now());
            vault.updated_at = Timestamp::now();
            self.store.replace_vault(&ctx.workspace, vault.clone()).await.map_err(map_store)?;
        }
        Ok(vault)
    }

    /// `DELETE /v1/vaults/{id}`.
    pub async fn delete_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        self.store.delete_vault(&ctx.workspace, &id).await.map_err(map_store)
    }

    // ── Credential CRUD ───────────────────────────────────────────────────

    /// `POST /v1/vaults/{id}/credentials`.
    pub async fn create_credential(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        req: CreateCredentialRequest,
    ) -> Result<Credential, ApiError> {
        let vault_id = parse_vault_id(raw_vault_id)?;
        let vault = self.store.get_vault(&ctx.workspace, &vault_id).await.map_err(map_store)?;
        if vault.archived_at.is_some() {
            return Err(ApiError::conflict_invalid_request("cannot add credentials to an archived vault"));
        }
        let display_name = required("display_name", req.display_name)?;
        let auth = required("auth", req.auth)?;
        validate_auth(&auth)?;
        let existing = self.store.list_credentials(&ctx.workspace, &vault_id).await.map_err(map_store)?;
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
        self.store
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
        self.store.get_credential(&ctx.workspace, &vid, &cid).await.map(|c| c.wire).map_err(map_store)
    }

    /// `GET /v1/vaults/{id}/credentials`.
    pub async fn list_credentials(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
    ) -> Result<Vec<Credential>, ApiError> {
        let vid = parse_vault_id(raw_vault_id)?;
        Ok(self
            .store
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
        let mut stored = self.store.get_credential(&ctx.workspace, &vid, &cid).await.map_err(map_store)?;
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
                (
                    Some(crate::types::CredentialAuthInput::McpOauth { .. }),
                    crate::types::CredentialAuthInput::McpOauth { .. }
                ) | (
                    Some(crate::types::CredentialAuthInput::StaticBearer { .. }),
                    crate::types::CredentialAuthInput::StaticBearer { .. }
                ) | (
                    Some(crate::types::CredentialAuthInput::EnvironmentVariable { .. }),
                    crate::types::CredentialAuthInput::EnvironmentVariable { .. }
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
        self.store.replace_credential(&ctx.workspace, &vid, stored.clone()).await.map_err(map_store)?;
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
        let mut stored = self.store.get_credential(&ctx.workspace, &vid, &cid).await.map_err(map_store)?;
        if stored.wire.archived_at.is_none() {
            stored.wire.archived_at = Some(Timestamp::now());
            stored.wire.updated_at = Timestamp::now();
            stored.auth = None; // secret purged
            self.store.replace_credential(&ctx.workspace, &vid, stored.clone()).await.map_err(map_store)?;
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
        self.store.delete_credential(&ctx.workspace, &vid, &cid).await.map(|c| c.wire).map_err(map_store)
    }
}

fn parse_vault_id(raw: &str) -> Result<VaultId, ApiError> {
    VaultId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn parse_pair(v: &str, c: &str) -> Result<(VaultId, CredentialId), ApiError> {
    let vid = parse_vault_id(v)?;
    let cid = CredentialId::parse(c).map_err(|e| ApiError::invalid_request(e.to_string()))?;
    Ok((vid, cid))
}

/// Live: env-var credentials REQUIRE `auth.networking` — a missing one is
/// `auth.networking: Field required` (an exact nested field-path message serde
/// can't produce, so it's validated here).
fn validate_auth(auth: &crate::types::CredentialAuthInput) -> Result<(), ApiError> {
    if let crate::types::CredentialAuthInput::EnvironmentVariable { networking: None, .. } = auth {
        return Err(ApiError::invalid_request("auth.networking: Field required"));
    }
    Ok(())
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::VaultNotFound => ApiError::not_found("vault not found"),
        StoreError::CredentialNotFound => ApiError::not_found("credential not found"),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
