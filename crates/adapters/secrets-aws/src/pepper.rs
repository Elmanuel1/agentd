//! The API key pepper: 32 random bytes, hex encoded, kept as the secret
//! `<prefix>/system/api_key_pepper` under the same KMS key as every
//! credential. It is created on first use; a name of two segments under the
//! prefix can never be a credential's, which has three.

use aws_sdk_secretsmanager::operation::create_secret::CreateSecretError;
use aws_sdk_secretsmanager::types::Tag;
use rand::RngCore;
use shared::ports::keys::KeyPepper;
use shared::ports::secrets::{ExposeSecret, SecretError, SecretString};
use zeroize::{Zeroize, Zeroizing};

use crate::failure::failure;
use crate::AwsSecretStore;

const NAME: &str = "system/api_key_pepper";

impl AwsSecretStore {
    /// The pepper API key digests are keyed with, created when there is none.
    /// A pepper scheduled for deletion is an error: restore it, or every key
    /// stops resolving.
    pub async fn api_key_pepper(&self) -> Result<KeyPepper, SecretError> {
        let name = format!("{}/{NAME}", self.prefix);
        match self.read(&name).await {
            Ok(value) => return decode(&name, &value),
            Err(SecretError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let mut fresh = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut fresh);
        let value = SecretString::from(Zeroizing::new(hex::encode(fresh.as_slice())).as_str());
        fresh.zeroize();
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
            Ok(_) => decode(&name, &value),
            // Another replica created it first.
            Err(e) if e.as_service_error().is_some_and(CreateSecretError::is_resource_exists_exception) => {
                decode(&name, &self.read(&name).await?)
            }
            Err(e)
                if e.as_service_error().is_some_and(CreateSecretError::is_invalid_request_exception)
                    && self.scheduled_for_deletion(&name).await? =>
            {
                Err(SecretError::Backend(format!(
                    "the API key pepper {name} is scheduled for deletion; restore it with RestoreSecret, \
                     or no API key resolves"
                )))
            }
            Err(e) => Err(failure("secretsmanager:CreateSecret", &name, &e)),
        }
    }
}

fn decode(name: &str, value: &SecretString) -> Result<KeyPepper, SecretError> {
    let mut bytes = [0u8; 32];
    let decoded = hex::decode_to_slice(value.expose_secret(), &mut bytes);
    let pepper = decoded.map(|()| KeyPepper::new(&bytes));
    bytes.zeroize();
    pepper.map_err(|_| SecretError::Backend(format!("{name} is not 32 hex-encoded bytes")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pepper_is_32_hex_bytes() {
        let pepper = decode("n", &SecretString::from("ab".repeat(32).as_str())).unwrap();
        assert_eq!(pepper.expose(), &[0xab; 32]);
        assert_eq!(format!("{pepper:?}"), "KeyPepper(***)");
        for bad in ["", "ab", &"zz".repeat(32), &"ab".repeat(33)] {
            let err = decode("n", &SecretString::from(bad)).unwrap_err().to_string();
            assert!(err.contains("not 32 hex-encoded bytes") && (bad.is_empty() || !err.contains(bad)));
        }
    }
}
