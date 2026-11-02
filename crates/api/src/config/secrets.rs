//! The secret store setting.

use secrets_aws::{AwsSecretsConfig, DEFAULT_PREFIX, DEFAULT_RECOVERY_WINDOW_DAYS};

use super::{flag, number, selector, unknown, ConfigError};

/// `AGENTD_SECRETS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsConfig {
    /// `aws`: one AWS Secrets Manager secret per credential, under
    /// `AGENTD_SECRETS_PREFIX`, encrypted with the customer-managed KMS key
    /// `AGENTD_SECRETS_KMS_KEY_ID`.
    Aws(AwsSecretsConfig),
    /// `blob`: secrets as objects in the blob store, not encrypted. Only
    /// with `AGENTD_DEV_SECRETS=1`.
    Blob,
}

impl SecretsConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let dev = flag(get, "AGENTD_DEV_SECRETS", false)?;
        match selector(get, "AGENTD_SECRETS", "aws").as_str() {
            "aws" => {
                let kms_key_id =
                    get("AGENTD_SECRETS_KMS_KEY_ID").filter(|k| !k.trim().is_empty()).ok_or(ConfigError {
                        var: "AGENTD_SECRETS_KMS_KEY_ID",
                        reason: "required with AGENTD_SECRETS=aws: the customer-managed KMS key secrets are \
                                 encrypted with"
                            .into(),
                    })?;
                let endpoint = get("AGENTD_SECRETS_ENDPOINT");
                if endpoint.as_deref().is_some_and(|e| !e.starts_with("https://")) && !dev {
                    return Err(ConfigError {
                        var: "AGENTD_SECRETS_ENDPOINT",
                        reason: "not https, so credentials would cross the network in plain text; set \
                                 AGENTD_DEV_SECRETS=1 to allow it for a local emulator"
                            .into(),
                    });
                }
                let prefix = get("AGENTD_SECRETS_PREFIX").unwrap_or_else(|| DEFAULT_PREFIX.into());
                let recovery_window_days =
                    number(get, "AGENTD_SECRETS_RECOVERY_DAYS", DEFAULT_RECOVERY_WINDOW_DAYS)?;
                if !(7..=30).contains(&recovery_window_days) {
                    return Err(ConfigError {
                        var: "AGENTD_SECRETS_RECOVERY_DAYS",
                        reason: format!("{recovery_window_days} is outside Secrets Manager's 7 to 30 days"),
                    });
                }
                Ok(Self::Aws(AwsSecretsConfig {
                    prefix,
                    kms_key_id,
                    region: get("AGENTD_SECRETS_REGION"),
                    endpoint,
                    role_arn: get("AGENTD_SECRETS_ROLE_ARN"),
                    recovery_window_days,
                }))
            }
            "blob" if dev => Ok(Self::Blob),
            "blob" => Err(ConfigError {
                var: "AGENTD_SECRETS",
                reason: "`blob` keeps credentials unencrypted in the blob store; use `aws`, or set \
                         AGENTD_DEV_SECRETS=1 in development"
                    .into(),
            }),
            other => Err(unknown("AGENTD_SECRETS", other)),
        }
    }
}
