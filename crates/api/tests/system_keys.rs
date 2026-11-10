//! The keys agentd signs with, read at boot from LocalStack's Secrets
//! Manager: the same on every boot, so a token minted before a restart
//! verifies after it; with `AGENTD_WORK_TOKEN_KEY` set, that key signs and
//! `system/work_token_key` is never read or made. Runs when
//! `AGENTD_AWS_ENDPOINT` names the emulator.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use api::config::SandboxConfig;
use api::SystemKeys;
use secrets_aws::{AwsSecretStore, AwsSecretsConfig};
use work::{WorkClaims, WorkTokens};

async fn aws(endpoint: &str, prefix: &str) -> AwsSecretStore {
    let shared = aws_common::sdk_config(None, None).await;
    let kms = aws_sdk_kms::Client::from_conf(
        aws_sdk_kms::config::Builder::from(&shared).endpoint_url(endpoint).build(),
    );
    let key = kms.create_key().send().await.unwrap().key_metadata.unwrap().key_id;
    let mut config = AwsSecretsConfig::new(prefix, key);
    config.endpoint = Some(endpoint.to_owned());
    AwsSecretStore::connect(&config).await.unwrap()
}

/// A prefix of the test's own: the tests run at once, so a timestamp alone can repeat.
fn prefix(test: &str) -> String {
    format!("agentd_test/{test}{}", shared::Timestamp::now().as_datetime().timestamp_micros())
}

#[tokio::test]
async fn a_token_minted_under_one_boot_verifies_under_the_next() {
    let Ok(endpoint) = std::env::var("AGENTD_AWS_ENDPOINT") else { return };
    let prefix = prefix("boot");
    let first = SystemKeys::load(&aws(&endpoint, &prefix).await, &SandboxConfig::default()).await.unwrap();
    let next = SystemKeys::load(&aws(&endpoint, &prefix).await, &SandboxConfig::default()).await.unwrap();
    assert_eq!((first.work_token_key.len(), first.egress_key.len()), (32, 32));
    let claims = WorkClaims { session: "sesn_1".into(), ws: "wrkspc_1".into(), org: "org_1".into() };
    let token = WorkTokens::new(first.work_token_key).mint(&claims);
    assert_eq!(WorkTokens::new(next.work_token_key).verify(&token), Some(claims));
    assert_eq!(first.egress_key, next.egress_key);
}

#[tokio::test]
async fn a_configured_work_token_key_signs_and_secrets_manager_is_not_read_for_it() {
    let Ok(endpoint) = std::env::var("AGENTD_AWS_ENDPOINT") else { return };
    let prefix = prefix("configured");
    let sandbox = SandboxConfig { work_token_key: Some(vec![7; 32]), ..SandboxConfig::default() };
    let keys = SystemKeys::load(&aws(&endpoint, &prefix).await, &sandbox).await.unwrap();
    assert_eq!(keys.work_token_key, vec![7; 32]);
    let shared = aws_common::sdk_config(None, None).await;
    let sm = aws_sdk_secretsmanager::Client::from_conf(
        aws_sdk_secretsmanager::config::Builder::from(&shared).endpoint_url(&endpoint).build(),
    );
    let described = sm.describe_secret().secret_id(format!("{prefix}/system/work_token_key")).send().await;
    assert!(described.is_err(), "no system/work_token_key was made");
    assert!(sm.describe_secret().secret_id(format!("{prefix}/system/egress_key")).send().await.is_ok());
}
