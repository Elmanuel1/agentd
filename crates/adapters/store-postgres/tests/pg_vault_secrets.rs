//! The vault store's `environment_variable` reads over live Postgres: a
//! session's credentials in one statement, and every such secret for
//! `agentd vault-secrets tag`. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use serde_json::json;
use shared::ports::secrets::SecretKey;
use shared::{AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgVaultsStore};
use test_support::MemorySecretStore;
use vaults::store::VaultCredentialStore;
use vaults::VaultsService;

#[tokio::test]
async fn environment_variable_credentials_are_listed_and_their_secrets_found_for_tagging() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let secrets = Arc::new(MemorySecretStore::default());
    let store = Arc::new(PgVaultsStore::new(PgBackend::connect(&url).await.unwrap(), secrets.clone()));
    let svc = VaultsService::new(store.clone(), store.clone());
    let ws = WorkspaceId::new(format!("wrkspc_tag{}", Timestamp::now().as_datetime().timestamp_micros()));
    let ctx =
        AuthContext { org: OrgId::new("org_tag"), workspace: ws.clone(), api_key_id: "apikey_tag".into() };
    let vault =
        svc.create_vault(&ctx, serde_json::from_value(json!({"display_name": "v"})).unwrap()).await.unwrap();
    let create =
        |auth: serde_json::Value| serde_json::from_value(json!({"display_name": "c", "auth": auth})).unwrap();
    let env = json!({"type": "environment_variable", "secret_name": "TOKEN", "secret_value": "v", "networking": {"type": "unrestricted"}});
    let bearer =
        json!({"type": "static_bearer", "mcp_server_url": "https://mcp.agentd.test/sse", "token": "t"});
    let env_cred = svc.create_credential(&ctx, vault.id.as_str(), create(env)).await.unwrap();
    let mcp_cred = svc.create_credential(&ctx, vault.id.as_str(), create(bearer)).await.unwrap();

    let listed = store.environment_credentials(&ws, &[vault.id.as_str().to_owned()]).await.unwrap();
    assert_eq!(listed.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), [env_cred.id.as_str()]);

    let keys = store.environment_variable_secrets().await.unwrap();
    let env_key = SecretKey::new(ws.as_str(), vault.id.as_str(), env_cred.id.as_str()).unwrap();
    let mcp_key = SecretKey::new(ws.as_str(), vault.id.as_str(), mcp_cred.id.as_str()).unwrap();
    assert!(
        keys.contains(&env_key.clone().environment_variable()),
        "the environment_variable secret is found"
    );
    assert!(!keys.iter().any(|k| k.to_string() == mcp_key.to_string()), "an MCP secret is not");
    assert!(keys.iter().all(SecretKey::is_environment_variable));
    assert!(secrets.tagged(&env_key) && !secrets.tagged(&mcp_key), "tagged when created");

    svc.archive_credential(&ctx, vault.id.as_str(), env_cred.id.as_str()).await.unwrap();
    assert!(store.environment_credentials(&ws, &[vault.id.as_str().to_owned()]).await.unwrap().is_empty());
    svc.delete_vault(&ctx, vault.id.as_str()).await.unwrap();
}
