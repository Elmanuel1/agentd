//! The API key pepper: 32 random bytes, hex encoded, kept as the system
//! secret `<prefix>/system/api_key_pepper`.

use shared::ports::keys::KeyPepper;
use shared::ports::secrets::SecretError;

use crate::system_secret::{bytes, random_hex};
use crate::AwsSecretStore;

impl AwsSecretStore {
    /// The pepper API key digests are keyed with, created when there is none.
    pub async fn api_key_pepper(&self) -> Result<KeyPepper, SecretError> {
        let value = self.system_secret("api_key_pepper", random_hex).await?;
        Ok(KeyPepper::new(&*bytes("api_key_pepper", &value)?))
    }
}
