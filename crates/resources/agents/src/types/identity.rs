use serde::{Deserialize, Serialize};

/// The principal an agent's runs execute as. `service_account` is the
/// default, and a request sending `null` restores it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionIdentity {
    /// The workspace's service account.
    #[default]
    ServiceAccount,
    /// An AWS IAM role.
    AwsRole {
        /// The role's ARN.
        role_arn: String,
    },
}
