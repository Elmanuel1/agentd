//! Tagging an existing `environment_variable` credential's secret, for the
//! secrets created before new ones carried the tag (`agentd vault-secrets tag`).

use aws_sdk_secretsmanager::types::Tag;
use shared::ports::secrets::{SecretError, SecretKey};

use crate::failure::failure;
use crate::name::CREDENTIAL_TYPE;
use crate::AwsSecretStore;

impl AwsSecretStore {
    /// Tag `key`'s secret `credential_type=environment_variable`; tagging
    /// one that already carries it changes nothing. `managed-by=agentd` goes
    /// with it, as agentd's IAM policy requires of every tag it writes.
    pub async fn tag_environment_variable(&self, key: &SecretKey) -> Result<(), SecretError> {
        let name = self.secret_name(key);
        let (k, v) = CREDENTIAL_TYPE;
        self.client
            .tag_resource()
            .secret_id(&name)
            .tags(Tag::builder().key(k).value(v).build())
            .tags(Tag::builder().key("managed-by").value("agentd").build())
            .send()
            .await
            .map_err(|e| failure("secretsmanager:TagResource", &name, &e))?;
        Ok(())
    }
}
