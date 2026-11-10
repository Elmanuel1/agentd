//! Vault business logic; the credential operations are in [`crate::credentials`].

use std::sync::Arc;

use shared::limits::{required, validate_string_len, values};
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, Pagination, Timestamp};

use crate::store::{StoreError, VaultCredentialStore, VaultStore};
use crate::types::{CreateVaultRequest, CredentialId, UpdateVaultRequest, Vault, VaultId, VaultTag};

/// Vault service.
pub struct VaultsService {
    pub(crate) vaults: Arc<dyn VaultStore>,
    pub(crate) credentials: Arc<dyn VaultCredentialStore>,
}

impl std::fmt::Debug for VaultsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultsService").finish_non_exhaustive()
    }
}

impl VaultsService {
    /// Build with the vault aggregate's two ports, usually one store.
    pub fn new(vaults: Arc<dyn VaultStore>, credentials: Arc<dyn VaultCredentialStore>) -> Self {
        Self { vaults, credentials }
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
        self.vaults.insert_vault(&ctx.workspace, vault.clone()).await.map_err(map_store)?;
        Ok(vault)
    }

    /// `GET /v1/vaults/{id}`.
    pub async fn get_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        self.vaults.get_vault(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/vaults`.
    pub async fn list_vaults(&self, ctx: &AuthContext, page: &Pagination) -> Result<Vec<Vault>, ApiError> {
        self.vaults.list_vaults(&ctx.workspace, page).await.map_err(map_store)
    }

    /// `POST /v1/vaults/{id}`.
    pub async fn update_vault(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        req: UpdateVaultRequest,
    ) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        let mut vault = self.vaults.get_vault(&ctx.workspace, &id).await.map_err(map_store)?;
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
        self.vaults.replace_vault(&ctx.workspace, vault.clone()).await.map_err(map_store)?;
        Ok(vault)
    }

    /// `POST /v1/vaults/{id}/archive`.
    pub async fn archive_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        let mut vault = self.vaults.get_vault(&ctx.workspace, &id).await.map_err(map_store)?;
        if vault.archived_at.is_none() {
            vault.archived_at = Some(Timestamp::now());
            vault.updated_at = Timestamp::now();
            self.vaults.replace_vault(&ctx.workspace, vault.clone()).await.map_err(map_store)?;
        }
        Ok(vault)
    }

    /// `DELETE /v1/vaults/{id}`.
    pub async fn delete_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        self.vaults.delete_vault(&ctx.workspace, &id).await.map_err(map_store)
    }
}

pub(crate) fn parse_vault_id(raw: &str) -> Result<VaultId, ApiError> {
    VaultId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

pub(crate) fn parse_pair(v: &str, c: &str) -> Result<(VaultId, CredentialId), ApiError> {
    let vid = parse_vault_id(v)?;
    let cid = CredentialId::parse(c).map_err(|e| ApiError::invalid_request(e.to_string()))?;
    Ok((vid, cid))
}

pub(crate) fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::VaultNotFound => ApiError::not_found("vault not found"),
        StoreError::CredentialNotFound => ApiError::not_found("credential not found"),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
