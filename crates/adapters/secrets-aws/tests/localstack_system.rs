//! System secrets against LocalStack: each is created once and read back the
//! same, so every replica and every boot signs with one key; an
//! `environment_variable` credential's secret carries the `credential_type`
//! tag. Runs when `AGENTD_AWS_ENDPOINT` names the emulator.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use secrets_aws::{SecretKey, SecretStore, SecretString};
use shared::ports::keys::material::{well_formed, KeyKind};
use shared::ports::secrets::ExposeSecret;

mod common;

use common::{endpoint, id, manager, store};

async fn tags(endpoint: &str, name: &str) -> Vec<(String, String)> {
    let out = manager(endpoint).await.describe_secret().secret_id(name).send().await.unwrap();
    out.tags
        .unwrap_or_default()
        .into_iter()
        .map(|t| (t.key.unwrap_or_default(), t.value.unwrap_or_default()))
        .collect()
}

#[tokio::test]
async fn the_work_token_egress_and_runtime_keys_are_created_once_and_read_back_the_same() {
    let Some(endpoint) = endpoint() else { return };
    let prefix = format!("agentd_test/{}", id("sys"));
    let first = store(&endpoint, &prefix).await;
    let (work, egress, runtime) = (
        first.work_token_key().await.unwrap(),
        first.egress_key().await.unwrap(),
        first.runtime_key().await.unwrap(),
    );
    assert_ne!(*work, *egress, "each key is its own");
    assert!(well_formed(KeyKind::Environment, runtime.expose_secret()), "an sk-ant-oat01- key");
    // A second boot, another replica: the same keys.
    let next = store(&endpoint, &prefix).await;
    assert_eq!(*next.work_token_key().await.unwrap(), *work);
    assert_eq!(*next.egress_key().await.unwrap(), *egress);
    assert_eq!(next.runtime_key().await.unwrap().expose_secret(), runtime.expose_secret());
    for name in ["work_token_key", "egress_key", "runtime_key"] {
        let tags = tags(&endpoint, &format!("{prefix}/system/{name}")).await;
        assert!(tags.contains(&("managed-by".into(), "agentd".into())), "{name}: {tags:?}");
    }
}

#[tokio::test]
async fn an_environment_variable_secret_is_tagged_when_created_and_an_mcp_one_is_not() {
    let Some(endpoint) = endpoint() else { return };
    let s = store(&endpoint, "agentd_test/it").await;
    let ws = id("wrkspc");
    let env_key = SecretKey::new(&ws, &id("vlt"), &id("cred")).unwrap().environment_variable();
    let mcp_key = SecretKey::new(&ws, &id("vlt"), &id("cred")).unwrap();
    s.put(&env_key, &SecretString::from("v1")).await.unwrap();
    s.put(&mcp_key, &SecretString::from("v2")).await.unwrap();
    let kind = ("credential_type".to_owned(), "environment_variable".to_owned());
    assert!(tags(&endpoint, &s.secret_name(&env_key)).await.contains(&kind));
    assert!(!tags(&endpoint, &s.secret_name(&mcp_key)).await.contains(&kind));
    // `agentd vault-secrets tag` tags one made before the tag existed, and again changes nothing.
    s.tag_environment_variable(&mcp_key).await.unwrap();
    s.tag_environment_variable(&mcp_key).await.unwrap();
    assert!(tags(&endpoint, &s.secret_name(&mcp_key)).await.contains(&kind));
}
