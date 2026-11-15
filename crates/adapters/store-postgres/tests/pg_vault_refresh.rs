//! The loop's MCP credential reads and its refresh claim over live Postgres:
//! the first vault holding a server's credential wins (B10), two claims of
//! one read make one winner (MC-7, MC-23), and a write after an archive
//! takes its secret back out. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use serde_json::json;
use shared::{AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgVaultsStore};
use test_support::MemorySecretStore;
use vaults::store::{RefreshWrite, VaultCredentialStore};
use vaults::types::VaultId;
use vaults::VaultsService;

#[tokio::test]
async fn one_claim_of_a_read_wins_and_the_first_vault_s_credential_is_used() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let store = Arc::new(PgVaultsStore::new(
        PgBackend::connect(&url).await.unwrap(),
        Arc::new(MemorySecretStore::default()),
    ));
    let svc = VaultsService::new(store.clone(), store.clone());
    let ws = WorkspaceId::new(format!("wrkspc_rf{}", Timestamp::now().as_datetime().timestamp_micros()));
    let ctx =
        AuthContext { org: OrgId::new("org_rf"), workspace: ws.clone(), api_key_id: "apikey_rf".into() };
    let vault = || async {
        svc.create_vault(&ctx, serde_json::from_value(json!({"display_name": "v"})).unwrap()).await.unwrap()
    };
    let (first, second) = (vault().await, vault().await);
    let server = "https://mcp.agentd.test/mcp";
    let oauth = json!({"display_name": "c", "auth": {"type": "mcp_oauth", "mcp_server_url": server,
        "access_token": "a", "refresh": {"client_id": "c", "token_endpoint": "https://auth.agentd.test/token",
        "refresh_token": "r"}}});
    let second_cred = svc
        .create_credential(&ctx, second.id.as_str(), serde_json::from_value(oauth.clone()).unwrap())
        .await
        .unwrap();
    let first_cred =
        svc.create_credential(&ctx, first.id.as_str(), serde_json::from_value(oauth).unwrap()).await.unwrap();
    let order = [first.id.as_str().to_owned(), second.id.as_str().to_owned()];
    let chosen = svc.mcp_credentials(&ws, &order, &[server.to_owned()]).await.unwrap();
    assert_eq!(chosen[server].id, first_cred.id, "the first of vault_ids wins");

    let vid = VaultId::parse(first.id.as_str()).unwrap();
    let now = Timestamp::now();
    assert!(store.claim_refresh(&ws, &vid, &first_cred.id, first_cred.updated_at, now).await.unwrap());
    assert!(
        !store
            .claim_refresh(&ws, &vid, &first_cred.id, first_cred.updated_at, Timestamp::now())
            .await
            .unwrap(),
        "a second claim of the same read matches nothing"
    );
    let mut stored = store.get_credential(&ws, &vid, &first_cred.id).await.unwrap();
    stored.wire.updated_at = Timestamp::now();
    assert_eq!(store.finish_refresh(&ws, &vid, &stored, now).await.unwrap(), RefreshWrite::Matched);
    let stale = stored.clone();
    assert_eq!(store.finish_refresh(&ws, &vid, &stale, now).await.unwrap(), RefreshWrite::Changed);

    svc.archive_credential(&ctx, first.id.as_str(), first_cred.id.as_str()).await.unwrap();
    assert_eq!(store.finish_refresh(&ws, &vid, &stale, now).await.unwrap(), RefreshWrite::Gone);
    let read = store.get_credential(&ws, &vid, &first_cred.id).await.unwrap();
    assert!(read.auth.is_none(), "the refresh's secret is taken back out");
    let left = svc.mcp_credentials(&ws, &order, &[server.to_owned()]).await.unwrap();
    assert_eq!(left[server].id, second_cred.id, "an archived credential is not used");
    svc.delete_vault(&ctx, first.id.as_str()).await.unwrap();
    svc.delete_vault(&ctx, second.id.as_str()).await.unwrap();
}
