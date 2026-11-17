//! The secret store setting: Postgres, the default, or AWS Secrets Manager.

use secrets_aws::{AwsSecretsConfig, DEFAULT_PREFIX, DEFAULT_RECOVERY_WINDOW_DAYS};
use shared::hash::SecretKeys;

use super::{endpoint, number, unknown, ConfigError};

/// `AGENTD_SECRETS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsConfig {
    /// `postgres`: each secret sealed in `vault_secrets` under the master
    /// keys `AGENTD_SECRET_KEYS`.
    Postgres(SecretKeys),
    /// `aws`: one Secrets Manager secret per credential, under
    /// `AGENTD_SECRETS_PREFIX`, encrypted with the customer-managed KMS key
    /// `AGENTD_SECRETS_KMS_KEY_ID`.
    Aws(AwsSecretsConfig),
}

/// The store `AGENTD_SECRETS` names; unset, `aws` when a KMS key is set and
/// `postgres` otherwise. A blank value counts as unset.
pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<SecretsConfig, ConfigError> {
    let set = |var: &str| get(var).filter(|v| !v.trim().is_empty());
    let default = if set("AGENTD_SECRETS_KMS_KEY_ID").is_some() { "aws" } else { "postgres" };
    match set("AGENTD_SECRETS").as_deref().unwrap_or(default) {
        "postgres" => {
            let var = "AGENTD_SECRET_KEYS";
            let reason = "required: a key ring `<id>:<hex>,…` of 32-byte keys".into();
            let raw = set(var).ok_or(ConfigError { var, reason })?;
            let keys = SecretKeys::parse(&raw).map_err(|e| ConfigError { var, reason: e.to_string() })?;
            Ok(SecretsConfig::Postgres(keys))
        }
        "aws" => aws(get).map(SecretsConfig::Aws),
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
    Ok(AwsSecretsConfig {
        prefix: get("AGENTD_SECRETS_PREFIX").unwrap_or_else(|| DEFAULT_PREFIX.into()),
        kms_key_id,
        region: get("AGENTD_SECRETS_REGION"),
        endpoint: endpoint(get, "AGENTD_SECRETS_ENDPOINT")?,
        role_arn: get("AGENTD_SECRETS_ROLE_ARN"),
        recovery_window_days,
    })
}
