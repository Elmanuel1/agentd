//! What the LocalStack tests share. They run when `AGENTD_AWS_ENDPOINT` names
//! the emulator, with any static credentials.

#![allow(dead_code, clippy::unwrap_used)]

use secrets_aws::{AwsSecretStore, AwsSecretsConfig};

pub fn endpoint() -> Option<String> {
    std::env::var("AGENTD_AWS_ENDPOINT").ok()
}

pub fn id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

pub async fn kms_key(endpoint: &str) -> String {
    let shared = aws_common::sdk_config(None, None).await;
    let conf = aws_sdk_kms::config::Builder::from(&shared).endpoint_url(endpoint).build();
    let out = aws_sdk_kms::Client::from_conf(conf).create_key().send().await.unwrap();
    out.key_metadata.unwrap().key_id
}

/// A store under `prefix`, with a fresh KMS key.
pub async fn store(endpoint: &str, prefix: &str) -> AwsSecretStore {
    let mut config = AwsSecretsConfig::new(prefix, kms_key(endpoint).await);
    config.endpoint = Some(endpoint.to_owned());
    AwsSecretStore::connect(&config).await.unwrap()
}

pub async fn manager(endpoint: &str) -> aws_sdk_secretsmanager::Client {
    let shared = aws_common::sdk_config(None, None).await;
    aws_sdk_secretsmanager::Client::from_conf(
        aws_sdk_secretsmanager::config::Builder::from(&shared).endpoint_url(endpoint).build(),
    )
}
