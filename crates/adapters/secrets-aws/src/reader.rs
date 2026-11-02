//! Read-only access to vault credentials, for the egress proxy: one
//! `GetSecretValue` per read, of the secret's `AWSCURRENT` version, and no
//! other call. Its role needs only that action on credential secrets.

use std::time::Duration;

use aws_sdk_secretsmanager::config::timeout::TimeoutConfig;
use aws_sdk_secretsmanager::operation::get_secret_value::GetSecretValueError;
use shared::ports::secrets::{SecretError, SecretKey, SecretString};

use crate::failure::failure;
use crate::name;
use crate::store::into_secret;

/// Reads credential secrets under one prefix; never writes, lists or caches.
#[derive(Clone)]
pub struct AwsSecretReader {
    client: aws_sdk_secretsmanager::Client,
    prefix: String,
}

impl std::fmt::Debug for AwsSecretReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsSecretReader").field("prefix", &self.prefix).finish_non_exhaustive()
    }
}

impl AwsSecretReader {
    /// A reader of secrets under `prefix`, in `region` (`None`: the default
    /// chain's), at `endpoint` (`None`: AWS), as `role_arn` when set.
    pub async fn connect(
        prefix: &str,
        region: Option<&str>,
        endpoint: Option<&str>,
        role_arn: Option<&str>,
    ) -> Result<Self, SecretError> {
        name::check_prefix(prefix)?;
        let shared = aws_common::sdk_config(region, role_arn).await;
        // A stalled read fails the one request it serves.
        let timeouts = TimeoutConfig::builder()
            .operation_attempt_timeout(Duration::from_secs(5))
            .operation_timeout(Duration::from_secs(10))
            .build();
        let mut sm = aws_sdk_secretsmanager::config::Builder::from(&shared).timeout_config(timeouts);
        if let Some(endpoint) = endpoint {
            sm = sm.endpoint_url(endpoint);
        }
        Ok(Self { client: aws_sdk_secretsmanager::Client::from_conf(sm.build()), prefix: prefix.to_owned() })
    }

    /// The current value of `key`. Every failure other than a missing secret
    /// is `Backend`, and the caller refuses what needed the value.
    pub async fn current(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        let name = name::secret_name(&self.prefix, key);
        match self.client.get_secret_value().secret_id(&name).version_stage("AWSCURRENT").send().await {
            Ok(out) => out
                .secret_string
                .map(into_secret)
                .ok_or_else(|| SecretError::Backend(format!("{name} holds no string value"))),
            Err(e)
                if e.as_service_error().is_some_and(GetSecretValueError::is_resource_not_found_exception) =>
            {
                Err(SecretError::NotFound(name))
            }
            Err(e) => Err(failure("secretsmanager:GetSecretValue", &name, &e)),
        }
    }
}
