//! A claimed write whose secret write fails after storing the value, as the
//! SSM store does when tagging fails: the secret of a credential whose vault
//! was archived since the claim goes (VA-3), a live credential's stays, and
//! the write's own error is returned, also when that cleanup fails. Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use shared::ports::secrets::{SecretError, SecretKey, SecretStore, SecretString};
use shared::{AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgVaultsStore};
use test_support::MemorySecretStore;
use vaults::store::{CredentialClaims, StoreError, StoredCredential};
use vaults::types::Credential;
use vaults::{VaultId, VaultsService};

/// Stores each put, then fails it while `refuse` is set; refuses each delete
/// while `refuse_delete` is set.
#[derive(Default)]
struct StoresThenFails {
    inner: MemorySecretStore,
    refuse: AtomicBool,
    refuse_delete: AtomicBool,
}

#[async_trait]
impl SecretStore for StoresThenFails {
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError> {
        self.inner.put(key, secret).await?;
        match self.refuse.load(Ordering::SeqCst) {
            true => Err(SecretError::Backend("ssm:AddTagsToResource on k failed: refused".into())),
            false => Ok(()),
        }
    }
    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        self.inner.get(key).await
    }
    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        match self.refuse_delete.load(Ordering::SeqCst) {
            true => Err(SecretError::Backend("ssm:DeleteParameter on k failed: refused".into())),
            false => self.inner.delete(key).await,
        }
    }
}

async fn vault(svc: &VaultsService, ctx: &AuthContext, name: &str) -> String {
    let req = serde_json::from_value(json!({"display_name": name})).unwrap();
    svc.create_vault(ctx, req).await.unwrap().id.as_str().to_owned()
}

async fn credential(svc: &VaultsService, ctx: &AuthContext, vault: &str) -> (VaultId, Credential) {
    let auth = json!({"type": "static_bearer", "mcp_server_url": "https://mcp.test/mcp", "token": "t"});
    let req = serde_json::from_value(json!({"display_name": "c", "auth": auth})).unwrap();
    (VaultId::parse(vault).unwrap(), svc.create_credential(ctx, vault, req).await.unwrap())
}

/// The claimed write of a new token, which the secret store refuses after storing it.
async fn refused_write(store: &PgVaultsStore, ws: &WorkspaceId, (vault, cred): &(VaultId, Credential)) {
    let auth = json!({"type": "static_bearer", "mcp_server_url": "https://mcp.test/mcp", "token": "t2"});
    let write = StoredCredential { wire: cred.clone(), auth: Some(serde_json::from_value(auth).unwrap()) };
    let err = store.write_claimed(ws, vault, &write, cred.updated_at).await.unwrap_err();
    assert!(matches!(err, StoreError::Backend(_)), "{err:?}");
    assert!(format!("{err:?}").contains("ssm:AddTagsToResource"), "the write's own error: {err:?}");
}

#[tokio::test]
async fn a_failed_claimed_write_keeps_no_secret_of_an_archived_credential() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let secrets = Arc::new(StoresThenFails::default());
    let store = Arc::new(PgVaultsStore::new(PgBackend::connect(&url).await.unwrap(), secrets.clone()));
    let svc = VaultsService::new(store.clone(), store.clone());
    let ws = WorkspaceId::new(format!("wrkspc_cf{}", Timestamp::now().as_datetime().timestamp_micros()));
    let ctx =
        AuthContext { org: OrgId::new("org_cf"), workspace: ws.clone(), api_key_id: "apikey_cf".into() };
    let (open, archived) = (vault(&svc, &ctx, "open").await, vault(&svc, &ctx, "archived").await);
    let (live, gone) = (credential(&svc, &ctx, &open).await, credential(&svc, &ctx, &archived).await);
    let secret = |(vault, cred): &(VaultId, Credential)| {
        secrets.inner.exposed(&SecretKey::new(ws.as_str(), vault.as_str(), cred.id.as_str()).unwrap())
    };
    svc.archive_vault(&ctx, &archived).await.unwrap();
    assert_eq!(secret(&gone), None);
    secrets.refuse.store(true, Ordering::SeqCst);

    secrets.refuse_delete.store(true, Ordering::SeqCst);
    refused_write(&store, &ws, &gone).await;
    assert!(secret(&gone).is_some(), "the refused delete left it, and the write's error is returned");
    secrets.refuse_delete.store(false, Ordering::SeqCst);

    refused_write(&store, &ws, &live).await;
    refused_write(&store, &ws, &gone).await;
    let held = secret(&live).unwrap();
    assert!(held.contains("\"t2\""), "a live credential keeps the value written: {held}");
    assert_eq!(secret(&gone), None, "an archived vault's credential keeps no secret");
    secrets.refuse.store(false, Ordering::SeqCst);
    for vault in [&open, &archived] {
        svc.delete_vault(&ctx, vault).await.unwrap();
    }
}
