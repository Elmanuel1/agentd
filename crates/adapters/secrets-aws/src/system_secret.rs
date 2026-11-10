//! System secrets: values agentd keeps once per deployment as
//! `<prefix>/system/<name>`, under the same KMS key as every credential, read
//! at boot and created the first time. A name of two segments under the
//! prefix can never be a credential's, which has three.

use aws_sdk_secretsmanager::operation::create_secret::CreateSecretError;
use aws_sdk_secretsmanager::types::Tag;
use rand::RngCore;
use shared::ports::keys::material::{self, KeyKind};
use shared::ports::secrets::{ExposeSecret, SecretError, SecretString};
use zeroize::{Zeroize, Zeroizing};

use crate::failure::failure;
use crate::AwsSecretStore;

impl AwsSecretStore {
    /// The value of `system/<name>`, created from `fresh` when there is none.
    /// One scheduled for deletion is an error: restore it, or what it keys
    /// stops working.
    pub(crate) async fn system_secret(
        &self,
        name: &str,
        fresh: impl FnOnce() -> SecretString,
    ) -> Result<SecretString, SecretError> {
        let name = format!("{}/system/{name}", self.prefix);
        match self.read(&name, &name).await {
            Err(SecretError::NotFound(_)) => {}
            read => return read,
        }
        let value = fresh();
        let created = self
            .client
            .create_secret()
            .name(&name)
            .kms_key_id(&self.key_arn)
            .secret_string(value.expose_secret())
            .client_request_token(uuid::Uuid::new_v4().to_string())
            .tags(Tag::builder().key("managed-by").value("agentd").build())
            .send()
            .await;
        match created {
            Ok(_) => Ok(value),
            // Another replica created it first.
            Err(e) if e.as_service_error().is_some_and(CreateSecretError::is_resource_exists_exception) => {
                self.read(&name, &name).await
            }
            Err(e)
                if e.as_service_error().is_some_and(CreateSecretError::is_invalid_request_exception)
                    && self.scheduled_for_deletion(&name, &name).await? =>
            {
                Err(SecretError::Backend(format!(
                    "{name} is scheduled for deletion; restore it with RestoreSecret"
                )))
            }
            Err(e) => Err(failure("secretsmanager:CreateSecret", &name, &e)),
        }
    }

    /// The key work tokens are signed with, `system/work_token_key`: the same
    /// on every replica and across restarts, so a token outlives the process
    /// that minted it.
    pub async fn work_token_key(&self) -> Result<Zeroizing<[u8; 32]>, SecretError> {
        bytes("work_token_key", &self.system_secret("work_token_key", random_hex).await?)
    }

    /// The key `environment_variable` placeholders are signed with,
    /// `system/egress_key`, which the egress swap reads to check them.
    pub async fn egress_key(&self) -> Result<Zeroizing<[u8; 32]>, SecretError> {
        bytes("egress_key", &self.system_secret("egress_key", random_hex).await?)
    }

    /// agentd's runtime key, `system/runtime_key`: the environment key that
    /// polls the runtime queue, minted on first boot.
    pub async fn runtime_key(&self) -> Result<SecretString, SecretError> {
        self.system_secret("runtime_key", || material::generate(KeyKind::Environment)).await
    }
}

/// 32 random bytes, hex encoded.
pub(crate) fn random_hex() -> SecretString {
    let mut fresh = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut fresh);
    let value = SecretString::from(Zeroizing::new(hex::encode(fresh.as_slice())).as_str());
    fresh.zeroize();
    value
}

/// The 32 bytes `value` holds, hex encoded.
pub(crate) fn bytes(name: &str, value: &SecretString) -> Result<Zeroizing<[u8; 32]>, SecretError> {
    let mut out = Zeroizing::new([0u8; 32]);
    hex::decode_to_slice(value.expose_secret(), out.as_mut())
        .map_err(|_| SecretError::Backend(format!("system/{name} is not 32 hex-encoded bytes")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_key_is_32_hex_bytes() {
        let fresh = random_hex();
        assert_eq!(bytes("k", &fresh).unwrap().len(), 32);
        assert_ne!(fresh.expose_secret(), random_hex().expose_secret());
        for bad in ["", "ab", &"zz".repeat(32), &"ab".repeat(33)] {
            let err = bytes("k", &SecretString::from(bad)).unwrap_err().to_string();
            assert!(err.contains("not 32 hex-encoded bytes") && (bad.is_empty() || !err.contains(bad)));
        }
    }
}
