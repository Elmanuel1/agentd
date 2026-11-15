//! A vault's archive and delete over live Postgres (VA-3, VA-7, VA-8): the
//! credentials go with the vault and their secrets are purged; a write into an
//! archived vault or credential is refused and leaves no secret. Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use serde_json::json;
use shared::ports::secrets::SecretKey;
use shared::{ApiError, AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgVaultsStore};
use test_support::MemorySecretStore;
use vaults::store::{StoreError, StoredCredential, VaultCredentialStore};
use vaults::types::Credential;
use vaults::VaultsService;

struct Rig {
    secrets: Arc<MemorySecretStore>,
    store: Arc<PgVaultsStore>,
    svc: VaultsService,
    ctx: AuthContext,
}

async fn rig() -> Option<Rig> {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return None;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let secrets = Arc::new(MemorySecretStore::default());
    let store = Arc::new(PgVaultsStore::new(PgBackend::connect(&url).await.unwrap(), secrets.clone()));
    let svc = VaultsService::new(store.clone(), store.clone());
    let ws = WorkspaceId::new(format!("wrkspc_va{}", Timestamp::now().as_datetime().timestamp_micros()));
    let ctx = AuthContext { org: OrgId::new("org_va"), workspace: ws, api_key_id: "apikey_va".into() };
    Some(Rig { secrets, store, svc, ctx })
}

impl Rig {
    async fn vault(&self) -> String {
        let req = serde_json::from_value(json!({"display_name": "v"})).unwrap();
        self.svc.create_vault(&self.ctx, req).await.unwrap().id.as_str().to_owned()
    }

    async fn credential(&self, vault: &str, url: &str) -> Result<Credential, ApiError> {
        let auth = json!({"type": "static_bearer", "mcp_server_url": url, "token": "t"});
        let req = serde_json::from_value(json!({"display_name": "c", "auth": auth})).unwrap();
        self.svc.create_credential(&self.ctx, vault, req).await
    }

    fn secret(&self, vault: &str, credential: &str) -> Option<String> {
        self.secrets.exposed(&SecretKey::new(self.ctx.workspace.as_str(), vault, credential).unwrap())
    }
}

#[tokio::test]
async fn archiving_a_vault_archives_its_credentials_and_purges_their_secrets() {
    // VA-3.
    let Some(rig) = rig().await else { return };
    let vault = rig.vault().await;
    let (one, two) = (
        rig.credential(&vault, "https://one.test/mcp").await.unwrap(),
        rig.credential(&vault, "https://two.test/mcp").await.unwrap(),
    );
    assert!(rig.secret(&vault, one.id.as_str()).is_some());
    let archived = rig.svc.archive_vault(&rig.ctx, &vault).await.unwrap();
    assert!(archived.archived_at.is_some());
    for c in [&one, &two] {
        let read = rig.svc.get_credential(&rig.ctx, &vault, c.id.as_str()).await.unwrap();
        assert_eq!(read.archived_at, archived.archived_at, "archived with the vault, at its time");
        assert_eq!(rig.secret(&vault, c.id.as_str()), None, "its secret is purged");
    }
    let again = rig.svc.archive_vault(&rig.ctx, &vault).await.unwrap();
    assert_eq!(again, archived, "an archived vault is answered as it is");
    rig.svc.delete_vault(&rig.ctx, &vault).await.unwrap();
}

#[tokio::test]
async fn a_write_into_an_archived_vault_or_credential_is_refused_and_leaves_no_secret() {
    // VA-7: the create's locked read refuses a vault archived since the service read it.
    let Some(rig) = rig().await else { return };
    let vault = rig.vault().await;
    let cred = rig.credential(&vault, "https://keep.test/mcp").await.unwrap();
    rig.svc.archive_vault(&rig.ctx, &vault).await.unwrap();
    let ws = rig.ctx.workspace.clone();
    let vault_id = vaults::VaultId::parse(&vault).unwrap();

    let mut late = cred.clone();
    late.id = vaults::types::CredentialId::generate();
    let auth = serde_json::from_value(
        json!({"type": "static_bearer", "mcp_server_url": "https://late.test/mcp", "token": "t"}),
    )
    .unwrap();
    let insert = rig
        .store
        .insert_credential(
            &ws,
            &rig.ctx.org,
            &vault_id,
            StoredCredential { wire: late.clone(), auth: Some(auth) },
        )
        .await;
    assert_eq!(insert, Err(StoreError::VaultArchived));
    assert_eq!(rig.secret(&vault, late.id.as_str()), None, "the create took its secret back out");

    let auth = serde_json::from_value(
        json!({"type": "static_bearer", "mcp_server_url": "https://keep.test/mcp", "token": "t2"}),
    )
    .unwrap();
    let rotate = rig
        .store
        .write_claimed(
            &ws,
            &vault_id,
            &StoredCredential { wire: cred.clone(), auth: Some(auth) },
            cred.updated_at,
        )
        .await;
    assert_eq!(rotate, Ok(vaults::store::ClaimedWrite::Gone));
    assert_eq!(rig.secret(&vault, cred.id.as_str()), None, "the rotation took its secret back out");
    rig.svc.delete_vault(&rig.ctx, &vault).await.unwrap();
}

#[tokio::test]
async fn deleting_a_vault_deletes_its_credentials_and_their_secrets() {
    // VA-8.
    let Some(rig) = rig().await else { return };
    let vault = rig.vault().await;
    let cred = rig.credential(&vault, "https://gone.test/mcp").await.unwrap();
    rig.svc.delete_vault(&rig.ctx, &vault).await.unwrap();
    assert_eq!(rig.secret(&vault, cred.id.as_str()), None);
    let read = rig.svc.get_credential(&rig.ctx, &vault, cred.id.as_str()).await.unwrap_err();
    assert_eq!(read.status, 404);
}

#[tokio::test]
async fn a_duplicate_key_is_refused_across_mcp_types_until_the_holder_is_archived() {
    // Q10.
    let Some(rig) = rig().await else { return };
    let vault = rig.vault().await;
    let held = rig.credential(&vault, "https://dup.test/mcp").await.unwrap();
    let auth = json!({"type": "mcp_oauth", "mcp_server_url": "https://dup.test/mcp", "access_token": "t"});
    let req = || serde_json::from_value(json!({"display_name": "o", "auth": auth})).unwrap();
    let refused = rig.svc.create_credential(&rig.ctx, &vault, req()).await.unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (409, "A credential already exists for this MCP server URL.")
    );
    rig.svc.archive_credential(&rig.ctx, &vault, held.id.as_str()).await.unwrap();
    rig.svc.create_credential(&rig.ctx, &vault, req()).await.unwrap();
    rig.svc.delete_vault(&rig.ctx, &vault).await.unwrap();
}
