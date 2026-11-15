//! A credential update while another writer moves the row under it: a lost
//! claim and a write after another claim each read the credential again, one
//! deleted meanwhile is `404`, and one that keeps changing is `409` after
//! three reads (VA-4, MC-7). Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unimplemented
)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use shared::{AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgVaultsStore};
use test_support::MemorySecretStore;
use vaults::store::{
    ClaimedWrite, CredentialClaims, CredentialReads, CredentialWrites, StoreError, StoredCredential,
    VaultCredentialStore,
};
use vaults::types::{Credential, CredentialAuthInput, CredentialId, SecretString, VaultId};
use vaults::VaultsService;
use webhooks::Delivery;

/// What the other writer does before the update's next store call.
#[derive(Debug, Clone, Copy)]
enum Race {
    /// Moves `updated_at` before the claim.
    Claim,
    /// Moves `updated_at` between the claim and the write.
    Write,
    /// Deletes the credential between the claim and the write.
    Delete,
}

/// The real store, with another writer acting at the points `races` names.
struct Racing {
    inner: Arc<PgVaultsStore>,
    races: Mutex<VecDeque<Race>>,
    reads: AtomicUsize,
}

impl Racing {
    fn next(&self, at: fn(Race) -> bool) -> Option<Race> {
        let mut races = self.races.lock().unwrap();
        races.front().copied().filter(|r| at(*r)).inspect(|_| {
            races.pop_front();
        })
    }
}

#[async_trait]
impl VaultCredentialStore for Racing {
    async fn get_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get_credential(ws, vault, id).await
    }
    async fn list_credentials(
        &self,
        _: &WorkspaceId,
        _: &VaultId,
    ) -> Result<Vec<StoredCredential>, StoreError> {
        unimplemented!()
    }
}

#[async_trait]
impl CredentialClaims for Racing {
    async fn claim_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
        read: Timestamp,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        if self.next(|r| matches!(r, Race::Claim)).is_some() {
            assert!(self.inner.claim_credential(ws, vault, id, read, Timestamp::now()).await.unwrap());
        }
        self.inner.claim_credential(ws, vault, id, read, now).await
    }
    async fn write_claimed(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: &StoredCredential,
        claimed: Timestamp,
    ) -> Result<ClaimedWrite, StoreError> {
        let id = &cred.wire.id;
        match self.next(|r| matches!(r, Race::Write | Race::Delete)) {
            Some(Race::Write) => {
                assert!(self.inner.claim_credential(ws, vault, id, claimed, Timestamp::now()).await.unwrap())
            }
            Some(_) => drop(self.inner.delete_credential(ws, vault, id, &[]).await.unwrap()),
            None => {}
        }
        self.inner.write_claimed(ws, vault, cred, claimed).await
    }
}

#[async_trait]
impl CredentialReads for Racing {
    async fn mcp_credentials(
        &self,
        _: &WorkspaceId,
        _: &[String],
        _: &[String],
    ) -> Result<Vec<Credential>, StoreError> {
        unimplemented!()
    }
    async fn credential_secret(
        &self,
        _: &WorkspaceId,
        _: &VaultId,
        _: &CredentialId,
    ) -> Result<CredentialAuthInput, StoreError> {
        unimplemented!()
    }
    async fn environment_credentials(
        &self,
        _: &WorkspaceId,
        _: &[String],
    ) -> Result<Vec<Credential>, StoreError> {
        unimplemented!()
    }
}

#[async_trait]
impl CredentialWrites for Racing {
    async fn insert_credential(
        &self,
        _: &WorkspaceId,
        _: &VaultId,
        _: StoredCredential,
        _: &[Delivery],
    ) -> Result<(), StoreError> {
        unimplemented!()
    }
    async fn archive_credential(
        &self,
        _: &WorkspaceId,
        _: &VaultId,
        _: StoredCredential,
        _: &[Delivery],
    ) -> Result<(), StoreError> {
        unimplemented!()
    }
    async fn delete_credential(
        &self,
        _: &WorkspaceId,
        _: &VaultId,
        _: &CredentialId,
        _: &[Delivery],
    ) -> Result<StoredCredential, StoreError> {
        unimplemented!()
    }
    async fn insert_deliveries(&self, _: &[Delivery]) -> Result<(), StoreError> {
        unimplemented!()
    }
}

#[tokio::test]
async fn an_update_reads_again_after_another_writer_and_gives_up_after_three_reads() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let store = Arc::new(PgVaultsStore::new(
        PgBackend::connect(&url).await.unwrap(),
        Arc::new(MemorySecretStore::default()),
    ));
    let plain = VaultsService::new(store.clone(), store.clone());
    let ws = WorkspaceId::new(format!("wrkspc_up{}", Timestamp::now().as_datetime().timestamp_micros()));
    let ctx =
        AuthContext { org: OrgId::new("org_up"), workspace: ws.clone(), api_key_id: "apikey_up".into() };
    let vault = plain
        .create_vault(&ctx, serde_json::from_value(json!({"display_name": "v"})).unwrap())
        .await
        .unwrap();
    let bearer = json!({"display_name": "c", "auth": {"type": "static_bearer",
        "mcp_server_url": "https://mcp.agentd.test/mcp", "token": "t0"}});
    let cred = plain
        .create_credential(&ctx, vault.id.as_str(), serde_json::from_value(bearer).unwrap())
        .await
        .unwrap();
    let vid = VaultId::parse(vault.id.as_str()).unwrap();
    let update = |token: &str| {
        serde_json::from_value(json!({"auth": {"type": "static_bearer", "token": token}})).unwrap()
    };
    let racing = |races: Vec<Race>| {
        let creds = Arc::new(Racing {
            inner: store.clone(),
            races: Mutex::new(races.into()),
            reads: AtomicUsize::new(0),
        });
        (VaultsService::new(store.clone(), creds.clone()), creds)
    };

    for (races, token) in [(vec![Race::Claim], "t1"), (vec![Race::Write], "t2")] {
        let (svc, creds) = racing(races.clone());
        let updated =
            svc.update_credential(&ctx, vault.id.as_str(), cred.id.as_str(), update(token)).await.unwrap();
        let stored = store.get_credential(&ws, &vid, &cred.id).await.unwrap();
        assert_eq!(updated, stored.wire, "{races:?}");
        assert_eq!(
            stored.auth,
            Some(CredentialAuthInput::StaticBearer {
                mcp_server_url: "https://mcp.agentd.test/mcp".into(),
                token: SecretString::new(token)
            })
        );
        assert_eq!(creds.reads.load(Ordering::SeqCst), 2, "read again once: {races:?}");
    }

    let (svc, creds) = racing(vec![Race::Claim, Race::Write, Race::Claim]);
    let err =
        svc.update_credential(&ctx, vault.id.as_str(), cred.id.as_str(), update("t3")).await.unwrap_err();
    assert_eq!(
        (err.status, err.message.as_str()),
        (409, "the credential changed during the update; send it again")
    );
    assert_eq!(creds.reads.load(Ordering::SeqCst), 3);

    let (svc, _) = racing(vec![Race::Delete]);
    let err =
        svc.update_credential(&ctx, vault.id.as_str(), cred.id.as_str(), update("t4")).await.unwrap_err();
    assert_eq!((err.status, err.message.as_str()), (404, "Credential not found."));
    assert!(matches!(store.get_credential(&ws, &vid, &cred.id).await, Err(StoreError::CredentialNotFound)));
    plain.delete_vault(&ctx, vault.id.as_str()).await.unwrap();
}
