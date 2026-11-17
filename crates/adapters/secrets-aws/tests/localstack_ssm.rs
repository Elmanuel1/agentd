//! `AwsParameterStore` against LocalStack SSM and KMS. Runs when
//! `AGENTD_AWS_ENDPOINT` names the emulator, with any static credentials and
//! `ssm` among LocalStack's services. LocalStack reports no tier, and treats
//! `alias/aws/ssm` as a customer key, and enforces no value size: the tier a
//! write names, the refusal of an AWS managed key and errors that carry no
//! value are unit tests (`ssm.rs`, `key.rs`, `failure.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use aws_sdk_ssm::types::{ParameterStringFilter, ParameterType, ResourceTypeForTagging};
use secrets_aws::{AwsParameterStore, AwsSecretsConfig, SecretError, SecretKey, SecretStore, SecretString};
use shared::ports::secrets::ExposeSecret;

mod common;

use common::{endpoint, id, kms_key};

fn key() -> SecretKey {
    SecretKey::new(&id("wrkspc"), &id("vlt"), &id("cred")).unwrap()
}

fn config(endpoint: &str, kms_key_id: String) -> AwsSecretsConfig {
    let mut config = AwsSecretsConfig::new("agentd_test/ssm", kms_key_id);
    config.endpoint = Some(endpoint.to_owned());
    config
}

async fn store(endpoint: &str) -> AwsParameterStore {
    AwsParameterStore::connect(&config(endpoint, kms_key(endpoint).await)).await.unwrap()
}

async fn ssm(endpoint: &str) -> aws_sdk_ssm::Client {
    let shared = aws_common::sdk_config(None, None).await;
    aws_sdk_ssm::Client::from_conf(aws_sdk_ssm::config::Builder::from(&shared).endpoint_url(endpoint).build())
}

/// The parameter's type and KMS key, as `DescribeParameters` reports them.
async fn described(endpoint: &str, name: &str) -> (Option<ParameterType>, Option<String>) {
    let filter = ParameterStringFilter::builder().key("Name").values(name).build().unwrap();
    let out = ssm(endpoint).await.describe_parameters().parameter_filters(filter).send().await.unwrap();
    let meta = out.parameters.unwrap_or_default().pop().unwrap();
    (meta.r#type, meta.key_id)
}

#[tokio::test]
async fn put_get_overwrite_delete_and_not_found_after() {
    let Some(endpoint) = endpoint() else { return };
    let store = store(&endpoint).await;
    let key = key();
    assert!(matches!(store.get(&key).await, Err(SecretError::NotFound(_))));
    store.delete(&key).await.unwrap();
    store.put(&key, &SecretString::from("first")).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "first");
    store.put(&key, &SecretString::from("second")).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "second");
    store.delete(&key).await.unwrap();
    let err = store.get(&key).await.unwrap_err();
    assert!(matches!(&err, SecretError::NotFound(name) if *name == store.parameter_name(&key)), "{err}");
    store.delete(&key).await.unwrap();
}

#[tokio::test]
async fn a_parameter_is_a_tagged_secure_string_under_the_key_after_an_overwrite_too() {
    let Some(endpoint) = endpoint() else { return };
    let store = store(&endpoint).await;
    let key = key().environment_variable();
    let name = store.parameter_name(&key);
    assert!(name.starts_with("/agentd_test/ssm/"), "{name}");
    store.put(&key, &SecretString::from("v1")).await.unwrap();
    store.put(&key, &SecretString::from("v2")).await.unwrap();
    let (kind, key_id) = described(&endpoint, &name).await;
    assert_eq!(kind, Some(ParameterType::SecureString));
    assert_eq!(key_id.as_deref(), Some(store.key_arn()), "the overwrite kept the customer key");
    let tags = ssm(&endpoint)
        .await
        .list_tags_for_resource()
        .resource_type(ResourceTypeForTagging::Parameter)
        .resource_id(&name)
        .send()
        .await
        .unwrap();
    let mut tags: Vec<_> =
        tags.tag_list.unwrap_or_default().iter().map(|t| format!("{}={}", t.key(), t.value())).collect();
    tags.sort();
    let mut want = vec![
        "credential_type=environment_variable".to_owned(),
        "managed-by=agentd".to_owned(),
        format!("vault={}", key.vault()),
        format!("workspace={}", key.workspace()),
    ];
    want.sort();
    assert_eq!(tags, want);
    store.delete(&key).await.unwrap();
}

#[tokio::test]
async fn connect_refuses_a_disabled_key_or_a_reserved_prefix() {
    let Some(endpoint) = endpoint() else { return };
    let disabled = kms_key(&endpoint).await;
    let shared = aws_common::sdk_config(None, None).await;
    let kms = aws_sdk_kms::Client::from_conf(
        aws_sdk_kms::config::Builder::from(&shared).endpoint_url(&endpoint).build(),
    );
    kms.disable_key().key_id(&disabled).send().await.unwrap();
    let err = AwsParameterStore::connect(&config(&endpoint, disabled)).await.unwrap_err().to_string();
    assert!(err.contains("not enabled"), "{err}");
    let mut reserved = config(&endpoint, kms_key(&endpoint).await);
    reserved.prefix = "AWS/agentd".into();
    let err = AwsParameterStore::connect(&reserved).await.unwrap_err().to_string();
    assert!(err.contains("reserves"), "{err}");
}

#[tokio::test]
async fn a_configured_timeout_reaches_the_clients() {
    // Gated with the other tests: the AWS credentials chain needs their static keys.
    if endpoint().is_none() {
        return;
    }
    // An endpoint that takes the connection and never answers, so only the timeout can end the call.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = silent.local_addr().unwrap();
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((conn, _)) = silent.accept().await {
            open.push(conn);
        }
    });
    let mut config = config(&format!("http://{address}"), "alias/agentd-timeout".to_owned());
    config.attempt_timeout = std::time::Duration::from_millis(100);
    config.operation_timeout = std::time::Duration::from_millis(300);
    let started = std::time::Instant::now();
    let err = AwsParameterStore::connect(&config).await.unwrap_err().to_string();
    assert!(err.ends_with("kms:DescribeKey on alias/agentd-timeout failed: timeout"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "the configured timeouts did not apply");
    held.abort();
}
