//! `AwsSecretStore` against LocalStack Secrets Manager and KMS. Runs when
//! `AGENTD_AWS_ENDPOINT` names the emulator, with any static credentials.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use aws_sdk_secretsmanager::types::Tag;
use secrets_aws::{AwsSecretStore, AwsSecretsConfig, SecretError, SecretKey, SecretStore, SecretString};
use shared::ports::secrets::ExposeSecret;

mod common;

use common::{endpoint, id, kms_key, manager};

fn key(workspace: &str) -> SecretKey {
    SecretKey::new(workspace, &id("vlt"), &id("cred")).unwrap()
}

async fn store(endpoint: &str) -> AwsSecretStore {
    common::store(endpoint, "agentd_test/it").await
}

fn secret(value: &str) -> SecretString {
    SecretString::from(value)
}

#[tokio::test]
async fn put_get_overwrite_delete_and_not_found_after() {
    let Some(endpoint) = endpoint() else { return };
    let store = store(&endpoint).await;
    let key = key(&id("wrkspc"));
    store.put(&key, &secret("first")).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "first");
    store.put(&key, &secret("second")).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "second");
    store.delete(&key).await.unwrap();
    let err = store.get(&key).await.unwrap_err();
    assert!(matches!(&err, SecretError::NotFound(name) if *name == store.secret_name(&key)), "{err}");
    store.delete(&key).await.unwrap();
    let never = self::key(&id("wrkspc"));
    assert!(matches!(store.get(&never).await, Err(SecretError::NotFound(_))));
    store.delete(&never).await.unwrap();
}

#[tokio::test]
async fn a_name_deleted_inside_its_recovery_window_is_written_again() {
    let Some(endpoint) = endpoint() else { return };
    let store = store(&endpoint).await;
    let key = key(&id("wrkspc"));
    store.put(&key, &secret("before")).await.unwrap();
    store.delete(&key).await.unwrap();
    let described =
        manager(&endpoint).await.describe_secret().secret_id(store.secret_name(&key)).send().await.unwrap();
    assert!(described.deleted_date.is_some(), "the delete keeps a recovery window");
    store.put(&key, &secret("after")).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "after");
}

#[tokio::test]
async fn a_secret_is_tagged_and_encrypted_under_the_configured_key() {
    let Some(endpoint) = endpoint() else { return };
    let store = store(&endpoint).await;
    let key = key(&id("wrkspc"));
    store.put(&key, &secret("v")).await.unwrap();
    let described =
        manager(&endpoint).await.describe_secret().secret_id(store.secret_name(&key)).send().await.unwrap();
    assert_eq!(described.name.as_deref(), Some(store.secret_name(&key).as_str()));
    assert!(store.secret_name(&key).starts_with("agentd_test/it/"));
    assert_eq!(described.kms_key_id.as_deref(), Some(store.key_arn()));
    let mut tags: Vec<(String, String)> = described
        .tags
        .unwrap_or_default()
        .into_iter()
        .map(|t: Tag| (t.key.unwrap_or_default(), t.value.unwrap_or_default()))
        .collect();
    tags.sort();
    let mut want = vec![
        ("managed-by".to_owned(), "agentd".to_owned()),
        ("vault".to_owned(), key.vault().to_owned()),
        ("workspace".to_owned(), key.workspace().to_owned()),
    ];
    want.sort();
    assert_eq!(tags, want);
}

#[tokio::test]
async fn one_workspace_never_reads_or_deletes_another_workspaces_secret() {
    let Some(endpoint) = endpoint() else { return };
    let store = store(&endpoint).await;
    let mine = key(&id("wrkspc"));
    let theirs = SecretKey::new(&id("wrkspc"), mine.vault(), mine.credential()).unwrap();
    store.put(&mine, &secret("mine")).await.unwrap();
    assert!(matches!(store.get(&theirs).await, Err(SecretError::NotFound(_))));
    store.delete(&theirs).await.unwrap();
    assert_eq!(store.get(&mine).await.unwrap().expose_secret(), "mine");
}

#[tokio::test]
async fn a_bad_id_or_prefix_is_rejected_before_any_call() {
    let Some(endpoint) = endpoint() else { return };
    for bad in ["../wrkspc_x", "wrkspc_x/vlt_y", "*", "", "wrkspc-x"] {
        assert!(matches!(SecretKey::new(bad, "vlt_a", "cred_a"), Err(SecretError::InvalidKey(_))), "{bad:?}");
        assert!(matches!(SecretKey::new("wrkspc_a", bad, "cred_a"), Err(SecretError::InvalidKey(_))));
        assert!(matches!(SecretKey::new("wrkspc_a", "vlt_a", bad), Err(SecretError::InvalidKey(_))));
    }
    for prefix in ["", "/", "a/../b", "a*", "a/"] {
        let mut config = AwsSecretsConfig::new(prefix, "unused");
        config.endpoint = Some(endpoint.clone());
        let err = AwsSecretStore::connect(&config).await.unwrap_err();
        assert!(err.to_string().contains("prefix"), "{prefix:?}: {err}");
    }
}

#[tokio::test]
async fn connect_fails_clearly_without_a_usable_key() {
    let Some(endpoint) = endpoint() else { return };
    let mut config = AwsSecretsConfig::new("agentd", "alias/agentd-missing-key");
    config.endpoint = Some(endpoint.clone());
    let err = AwsSecretStore::connect(&config).await.unwrap_err().to_string();
    assert!(err.contains("kms:DescribeKey on alias/agentd-missing-key failed: NotFoundException"), "{err}");
    config.kms_key_id = " ".into();
    let err = AwsSecretStore::connect(&config).await.unwrap_err().to_string();
    assert!(err.contains("KMS key id is required"), "{err}");
    config.kms_key_id = kms_key(&endpoint).await;
    config.recovery_window_days = 0;
    let err = AwsSecretStore::connect(&config).await.unwrap_err().to_string();
    assert!(err.contains("7 to 30 days"), "{err}");
}
