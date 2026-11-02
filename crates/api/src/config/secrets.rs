//! The secret store setting: AWS Secrets Manager, the only one there is.

use secrets_aws::{AwsSecretsConfig, DEFAULT_PREFIX, DEFAULT_RECOVERY_WINDOW_DAYS};

use super::{endpoint, number, selector, unknown, ConfigError};

/// `AGENTD_SECRETS=aws`: one Secrets Manager secret per credential, under
/// `AGENTD_SECRETS_PREFIX`, encrypted with the customer-managed KMS key
/// `AGENTD_SECRETS_KMS_KEY_ID`.
pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<AwsSecretsConfig, ConfigError> {
    match selector(get, "AGENTD_SECRETS", "aws").as_str() {
        "aws" => {}
        other => return Err(unknown("AGENTD_SECRETS", other)),
    }
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
