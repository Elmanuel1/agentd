//! A vault's archive and delete, each taking its credentials with it and
//! sending one event for the vault and one for each credential (B29, B30).

use shared::{ApiError, AuthContext, Timestamp};
use webhooks::{Delivery, EventKind};

use crate::service::{map_store, parse_vault_id, VaultsService};
use crate::types::{Vault, VaultId};

impl VaultsService {
    /// `POST /v1/vaults/{id}/archive`: its credentials are archived with it
    /// and their secrets purged (VA-3).
    pub async fn archive_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        let cascade = self.cascade(ctx, &id, EventKind::VaultArchived, |vault_id| {
            EventKind::CredentialArchived { vault_id }
        });
        self.vaults.archive_vault(&ctx.workspace, &id, Timestamp::now(), &cascade).await.map_err(map_store)
    }

    /// `DELETE /v1/vaults/{id}`.
    pub async fn delete_vault(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vault, ApiError> {
        let id = parse_vault_id(raw_id)?;
        let cascade = self
            .cascade(ctx, &id, EventKind::VaultDeleted, |vault_id| EventKind::CredentialDeleted { vault_id });
        self.vaults.delete_vault(&ctx.workspace, &id, &cascade).await.map_err(map_store)
    }

    /// The vault's event, and one for each credential the cascade took,
    /// archived ones included.
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
