//! Vault business logic; the credential operations are in [`crate::credentials`].

use std::sync::Arc;

use shared::limits::{required, validate_string_len, values};
use shared::metadata::MetadataPolicy;
use shared::{ApiError, AuthContext, OrgId, Pagination, Timestamp, WorkspaceId};
use webhooks::{Delivery, EventKind, Webhooks};

use crate::store::{StoreError, VaultCredentialStore, VaultStore};
use crate::types::{CreateVaultRequest, CredentialId, UpdateVaultRequest, Vault, VaultId, VaultTag};
use crate::validate::McpProbe;
use shared::net::Guard;

/// Vault service.
pub struct VaultsService {
    pub(crate) vaults: Arc<dyn VaultStore>,
    pub(crate) credentials: Arc<dyn VaultCredentialStore>,
    pub(crate) mcp: Option<McpReach>,
    webhooks: Arc<Webhooks>,
}

/// How the service reaches MCP servers and token endpoints itself.
#[derive(Clone)]
pub struct McpReach {
    /// Sends `initialize`.
    pub probe: Arc<dyn McpProbe>,
    /// Where token requests may go.
    pub guard: Guard,
}

impl std::fmt::Debug for McpReach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpReach").field("guard", &self.guard).finish_non_exhaustive()
    }
}

impl std::fmt::Debug for VaultsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultsService").finish_non_exhaustive()
    }
}

impl VaultsService {
    /// Build with the vault aggregate's two ports, usually one store.
    pub fn new(vaults: Arc<dyn VaultStore>, credentials: Arc<dyn VaultCredentialStore>) -> Self {
        Self { vaults, credentials, mcp: None, webhooks: Arc::default() }
    }

    /// The service, sending each vault and credential lifecycle event, and
    /// each failed refresh, to `webhooks`' endpoints in the write's own
    /// transaction (H-5, H-6).
    #[must_use]
    pub fn with_webhooks(mut self, webhooks: Arc<Webhooks>) -> Self {
        self.webhooks = webhooks;
        self
    }

    /// The deliveries of `kind` about `id` to `org`'s endpoints.
    pub(crate) fn deliveries(
        &self,
        org: &OrgId,
        ws: &WorkspaceId,
        kind: EventKind,
        id: &str,
    ) -> Vec<Delivery> {
        self.webhooks.deliveries(org.as_str(), ws.as_str(), kind, id)
    }

    /// The service, reaching MCP servers through `mcp`.
    #[must_use]
    pub fn with_mcp(mut self, mcp: McpReach) -> Self {
        self.mcp = Some(mcp);
        self
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
        let deliveries =
            self.deliveries(&ctx.org, &ctx.workspace, EventKind::VaultCreated, vault.id.as_str());
        self.vaults.insert_vault(&ctx.workspace, vault.clone(), &deliveries).await.map_err(map_store)?;
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
            return Err(ApiError::invalid_request("Vault is archived."));
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

    /// `POST /v1/vaults/{id}/archive`: its credentials are archived with it
    /// and their secrets purged (VA-3).
    pub async fn archive_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        let cascade = self.cascade(ctx, &id, EventKind::VaultArchived, |vault_id| {
            EventKind::CredentialArchived { vault_id }
        });
        self.vaults.archive_vault(&ctx.workspace, &id, Timestamp::now(), &cascade).await.map_err(map_store)
    }

    /// Refuse a session that names a vault it may not use (VA-2): live's 404
    /// for one not in the workspace, 400 for an archived one, first in order.
    pub async fn check_usable(&self, ctx: &AuthContext, raw_ids: &[String]) -> Result<(), ApiError> {
        let ids: Vec<VaultId> = raw_ids.iter().filter_map(|raw| VaultId::parse(raw).ok()).collect();
        let found = self.vaults.named_vaults(&ctx.workspace, &ids).await.map_err(map_store)?;
        for raw in raw_ids {
            match found.iter().find(|v| v.id.as_str() == raw) {
                None => return Err(ApiError::not_found(format!("vault {raw} not found"))),
                Some(v) if v.archived_at.is_some() => {
                    return Err(ApiError::invalid_request(format!("vault {raw} is archived")))
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// `DELETE /v1/vaults/{id}`.
    pub async fn delete_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        let cascade = self
            .cascade(ctx, &id, EventKind::VaultDeleted, |vault_id| EventKind::CredentialDeleted { vault_id });
        self.vaults.delete_vault(&ctx.workspace, &id, &cascade).await.map_err(map_store)
    }

    /// A cascade's deliveries: the vault's event, and one for each credential it
    /// took, archived ones included (B29, B30).
    fn cascade<'a>(
        &'a self,
        ctx: &'a AuthContext,
        id: &'a VaultId,
        vault: EventKind,
        credential: fn(String) -> EventKind,
    ) -> impl Fn(&[String]) -> Vec<Delivery> + Send + Sync + 'a {
        move |credentials: &[String]| {
            let mut deliveries = self.deliveries(&ctx.org, &ctx.workspace, vault.clone(), id.as_str());
            for c in credentials {
                let kind = credential(id.as_str().to_owned());
                deliveries.extend(self.deliveries(&ctx.org, &ctx.workspace, kind, c));
            }
            deliveries
        }
    }
}

/// Live answers a malformed vault id with its generic message.
pub(crate) fn parse_vault_id(raw: &str) -> Result<VaultId, ApiError> {
    VaultId::parse(raw).map_err(|_| ApiError::invalid_request("The request was invalid"))
}

pub(crate) fn parse_pair(v: &str, c: &str) -> Result<(VaultId, CredentialId), ApiError> {
    let vid = parse_vault_id(v)?;
    let cid = CredentialId::parse(c).map_err(|_| {
        ApiError::invalid_request(format!("Invalid credential_id \"{c}\": expected vcrd_ prefix."))
    })?;
    Ok((vid, cid))
}

pub(crate) fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::VaultNotFound => ApiError::not_found("Vault not found."),
        StoreError::CredentialNotFound => ApiError::not_found("Credential not found."),
        StoreError::VaultArchived => ApiError::invalid_request("Vault is archived."),
        StoreError::CredentialArchived => ApiError::invalid_request("Credential is archived."),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
