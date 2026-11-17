//! The secret store setting: Postgres, the default, AWS Secrets Manager or
//! SSM Parameter Store.

use std::time::Duration;

use secrets_aws::{
    AwsSecretsConfig, DEFAULT_ATTEMPT_TIMEOUT, DEFAULT_OPERATION_TIMEOUT, DEFAULT_PREFIX,
    DEFAULT_RECOVERY_WINDOW_DAYS,
};
use shared::hash::SecretKeys;

use super::{endpoint, number, unknown, ConfigError};

/// `AGENTD_SECRETS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsConfig {
    /// `postgres`: each secret sealed in `vault_secrets` under the master
    /// keys `AGENTD_SECRET_KEYS`.
    Postgres(SecretKeys),
    /// `aws_sm` (`aws` too): one Secrets Manager secret per credential, under
    /// `AGENTD_SECRETS_PREFIX`, encrypted with the customer-managed KMS key
    /// `AGENTD_SECRETS_KMS_KEY_ID`.
    AwsSm(AwsSecretsConfig),
    /// `aws_ssm`: one SecureString parameter per credential, under the same
    /// prefix and key settings, the prefix checked at connect; a parameter
    /// has no recovery window.
    AwsSsm(AwsSecretsConfig),
}

/// The store `AGENTD_SECRETS` names; unset, `aws_sm` when a KMS key is set and
/// `postgres` otherwise. A blank value counts as unset.
pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<SecretsConfig, ConfigError> {
    let set = |var: &str| get(var).filter(|v| !v.trim().is_empty());
    let default = match set("AGENTD_SECRETS_KMS_KEY_ID") {
        Some(_) => "aws_sm",
        None => "postgres",
    };
    match set("AGENTD_SECRETS").as_deref().unwrap_or(default) {
        "postgres" => {
            let var = "AGENTD_SECRET_KEYS";
            let reason = "required: a key ring `<id>:<hex>,…` of 32-byte keys".into();
            let raw = set(var).ok_or(ConfigError { var, reason })?;
            let keys = SecretKeys::parse(&raw).map_err(|e| ConfigError { var, reason: e.to_string() })?;
            Ok(SecretsConfig::Postgres(keys))
        }
        "aws_sm" | "aws" => aws(get).map(SecretsConfig::AwsSm),
        "aws_ssm" => {
            if let Some(days) = set("AGENTD_SECRETS_RECOVERY_DAYS") {
                let reason = format!("`{days}`: Parameter Store has no recovery window");
                return Err(ConfigError { var: "AGENTD_SECRETS_RECOVERY_DAYS", reason });
            }
            aws(get).map(SecretsConfig::AwsSsm)
        }
        other => Err(unknown("AGENTD_SECRETS", other)),
    }
}

fn aws(get: &impl Fn(&str) -> Option<String>) -> Result<AwsSecretsConfig, ConfigError> {
    let kms_key_id =
        get("AGENTD_SECRETS_KMS_KEY_ID").filter(|k| !k.trim().is_empty()).ok_or_else(|| ConfigError {
            var: "AGENTD_SECRETS_KMS_KEY_ID",
            reason: "required: the customer-managed KMS key secrets are encrypted with".into(),
        })?;
    let recovery_window_days = number(get, "AGENTD_SECRETS_RECOVERY_DAYS", DEFAULT_RECOVERY_WINDOW_DAYS)?;
    if !(7..=30).contains(&recovery_window_days) {
        return Err(ConfigError {
            var: "AGENTD_SECRETS_RECOVERY_DAYS",
            reason: format!("{recovery_window_days} is outside Secrets Manager's 7 to 30 days"),
        });
    }
    let ms = |var, default: Duration| {
        number(get, var, u64::try_from(default.as_millis()).unwrap_or(u64::MAX)).map(Duration::from_millis)
    };
    let attempt_timeout = ms("AGENTD_SECRETS_ATTEMPT_TIMEOUT_MS", DEFAULT_ATTEMPT_TIMEOUT)?;
    let operation_timeout = ms("AGENTD_SECRETS_TIMEOUT_MS", DEFAULT_OPERATION_TIMEOUT)?;
    if attempt_timeout > operation_timeout {
        let reason = format!("{attempt_timeout:?} is longer than the whole call's {operation_timeout:?}");
        return Err(ConfigError { var: "AGENTD_SECRETS_ATTEMPT_TIMEOUT_MS", reason });
    }
    Ok(AwsSecretsConfig {
        prefix: get("AGENTD_SECRETS_PREFIX").unwrap_or_else(|| DEFAULT_PREFIX.into()),
        kms_key_id,
        region: get("AGENTD_SECRETS_REGION"),
        endpoint: endpoint(get, "AGENTD_SECRETS_ENDPOINT")?,
        role_arn: get("AGENTD_SECRETS_ROLE_ARN"),
        recovery_window_days,
        attempt_timeout,
        operation_timeout,
    })
}
