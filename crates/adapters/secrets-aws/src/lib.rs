//! secrets-aws: the AWS Secrets Manager implementation of [`SecretStore`].
//!
//! Each credential is one secret, `<prefix>/<workspace>/<vault>/<credential>`,
//! encrypted under one customer-managed KMS key and tagged with its workspace,
//! its vault and `managed-by=agentd`. Credentials and region come from
//! [`aws_common::sdk_config`]. Secret values cross this crate only as
//! [`SecretString`], and no error it returns carries a value or an AWS error
//! message: only the secret name and the AWS error code. Nothing is cached:
//! every read is a `GetSecretValue`, so a rotated value is seen at once and
//! plaintext lives in memory only as long as its caller holds it.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

mod failure;
mod key;
mod name;
mod pepper;
mod store;

use std::time::Duration;

use aws_sdk_secretsmanager::config::timeout::TimeoutConfig;
pub use shared::ports::secrets::{SecretError, SecretKey, SecretStore, SecretString};

/// The prefix every secret name starts with unless configured otherwise.
pub const DEFAULT_PREFIX: &str = "agentd";
/// Days a deleted secret stays recoverable unless configured otherwise: the
/// shortest window Secrets Manager allows, nearest to the purge an archive
/// promises.
pub const DEFAULT_RECOVERY_WINDOW_DAYS: i64 = 7;

/// Where and how to reach Secrets Manager and the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsSecretsConfig {
    /// The first segments of every secret name, `/`-separated.
    pub prefix: String,
    /// The customer-managed KMS key every secret is encrypted under: a key
    /// id, key ARN, alias name or alias ARN. There is no fallback to the
    /// `aws/secretsmanager` key.
    pub kms_key_id: String,
    /// The region; `None` takes the default chain's.
    pub region: Option<String>,
    /// A custom endpoint for Secrets Manager and KMS (LocalStack); `None` is AWS.
    pub endpoint: Option<String>,
    /// A role to assume with the default-chain credentials.
    pub role_arn: Option<String>,
    /// Days a deleted secret stays recoverable, 7 to 30.
    pub recovery_window_days: i64,
}

impl AwsSecretsConfig {
    /// Secrets under `prefix`, encrypted under `kms_key_id`, in the
    /// default-chain region on AWS, with the default recovery window.
    pub fn new(prefix: impl Into<String>, kms_key_id: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            kms_key_id: kms_key_id.into(),
            region: None,
            endpoint: None,
            role_arn: None,
            recovery_window_days: DEFAULT_RECOVERY_WINDOW_DAYS,
        }
    }
}

/// Secrets in AWS Secrets Manager, under one prefix and one KMS key.
#[derive(Clone)]
pub struct AwsSecretStore {
    client: aws_sdk_secretsmanager::Client,
    prefix: String,
    key_arn: String,
    recovery_window_days: i64,
}

impl std::fmt::Debug for AwsSecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsSecretStore")
            .field("prefix", &self.prefix)
            .field("key_arn", &self.key_arn)
            .field("recovery_window_days", &self.recovery_window_days)
            .finish_non_exhaustive()
    }
}

impl AwsSecretStore {
    /// Check `config`, and that its KMS key is reachable and fit to encrypt
    /// secrets, then build the client. Fails before any secret call when
    /// either is not.
    pub async fn connect(config: &AwsSecretsConfig) -> Result<Self, SecretError> {
        name::check_prefix(&config.prefix)?;
        if !(7..=30).contains(&config.recovery_window_days) {
            return Err(SecretError::Backend(format!(
                "the recovery window must be 7 to 30 days, got {}",
                config.recovery_window_days
            )));
        }
        let shared = aws_common::sdk_config(config.region.as_deref(), config.role_arn.as_deref()).await;
        // A stalled call fails the request it serves instead of holding it.
        let timeouts = TimeoutConfig::builder()
            .operation_attempt_timeout(Duration::from_secs(5))
            .operation_timeout(Duration::from_secs(15))
            .build();
        let mut sm = aws_sdk_secretsmanager::config::Builder::from(&shared).timeout_config(timeouts.clone());
        let mut kms = aws_sdk_kms::config::Builder::from(&shared).timeout_config(timeouts);
        if let Some(endpoint) = &config.endpoint {
            sm = sm.endpoint_url(endpoint);
            kms = kms.endpoint_url(endpoint);
        }
        let kms = aws_sdk_kms::Client::from_conf(kms.build());
        let key_arn = key::customer_key_arn(&kms, &config.kms_key_id).await?;
        Ok(Self {
            client: aws_sdk_secretsmanager::Client::from_conf(sm.build()),
            prefix: config.prefix.clone(),
            key_arn,
            recovery_window_days: config.recovery_window_days,
        })
    }

    /// The KMS key ARN every secret this store creates is encrypted under.
    pub fn key_arn(&self) -> &str {
        &self.key_arn
    }

    /// The secret name of `key`.
    pub fn secret_name(&self, key: &SecretKey) -> String {
        name::secret_name(&self.prefix, key)
    }
}
