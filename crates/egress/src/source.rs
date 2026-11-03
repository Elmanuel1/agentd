//! A vault credential, read from Secrets Manager when a request needs it:
//! its sealed material is unsealed, its value copied into locked memory, and
//! every other copy zeroed when dropped.

use async_trait::async_trait;
use egress_proxy::{Current, Locked, SecretSource};
use secrets_aws::AwsSecretReader;
use shared::ports::secrets::{SecretError, SecretKey};
use vaults::types::{CredentialAuthInput, CredentialNetworking};

/// The credentials in Secrets Manager.
#[derive(Debug)]
pub struct VaultSource(pub AwsSecretReader);

#[async_trait]
impl SecretSource for VaultSource {
    async fn current(&self, key: &SecretKey) -> Result<Current, SecretError> {
        let sealed = self.0.current(key).await?;
        let auth = CredentialAuthInput::unseal(&sealed).map_err(|e| SecretError::Backend(e.to_string()))?;
        drop(sealed);
        let CredentialAuthInput::EnvironmentVariable { secret_value, networking, injection_location, .. } =
            auth
        else {
            return Err(SecretError::Backend(format!("{key} is not an environment variable credential")));
        };
        let value =
            Locked::new(secret_value.expose().as_bytes()).map_err(|e| SecretError::Backend(e.to_string()))?;
        Ok(Current {
            value,
            hosts: match networking {
                Some(CredentialNetworking::Limited { allowed_hosts }) => Some(allowed_hosts),
                Some(CredentialNetworking::Unrestricted) | None => None,
            },
            header: injection_location.unwrap_or_default().header,
        })
    }
}
