//! A claimed write whose secret write fails after storing the value, as the
//! SSM store does when tagging fails: the secret of a credential archived
//! since the claim goes, a live credential's stays, and the write's error is
//! returned. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

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
use vaults::VaultsService;

/// Stores each put, then fails it while `refuse` is set.
#[derive(Default)]
struct StoresThenFails {
    inner: MemorySecretStore,
    refuse: AtomicBool,
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
        self.inner.delete(key).await
    }
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
    let req = serde_json::from_value(json!({"display_name": "v"})).unwrap();
    let vault = svc.create_vault(&ctx, req).await.unwrap().id.as_str().to_owned();
    let vault_id = vaults::VaultId::parse(&vault).unwrap();
    let secret = |id: &str| secrets.inner.exposed(&SecretKey::new(ws.as_str(), &vault, id).unwrap());
    let auth = |url: &str, token: &str| {
        serde_json::from_value(json!({"type": "static_bearer", "mcp_server_url": url, "token": token}))
            .unwrap()
    };
    let credential = |url: &'static str| {
        let req = json!({"display_name": "c", "auth": {"type": "static_bearer", "mcp_server_url": url, "token": "t"}});
        svc.create_credential(&ctx, &vault, serde_json::from_value(req).unwrap())
    };
    let (live, gone) = (
        credential("https://live.test/mcp").await.unwrap(),
        credential("https://gone.test/mcp").await.unwrap(),
    );
    svc.archive_credential(&ctx, &vault, gone.id.as_str()).await.unwrap();
    assert_eq!(secret(gone.id.as_str()), None);
    secrets.refuse.store(true, Ordering::SeqCst);

    for (cred, url) in [(&live, "https://live.test/mcp"), (&gone, "https://gone.test/mcp")] {
        let write = StoredCredential { wire: cred.clone(), auth: Some(auth(url, "t2")) };
        let err = store.write_claimed(&ws, &vault_id, &write, cred.updated_at).await.unwrap_err();
        assert!(matches!(err, StoreError::Backend(_)), "{err:?}");
        assert!(format!("{err:?}").contains("ssm:AddTagsToResource"), "the write's own error: {err:?}");
    }
    let held = secret(live.id.as_str()).unwrap();
    assert!(held.contains("\"t2\""), "a live credential keeps the value written: {held}");
    assert_eq!(secret(gone.id.as_str()), None, "an archived credential keeps no secret");
    secrets.refuse.store(false, Ordering::SeqCst);
    svc.delete_vault(&ctx, &vault).await.unwrap();
}
