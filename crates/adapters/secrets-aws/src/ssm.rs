//! [`SecretStore`] on SSM Parameter Store: each credential one SecureString
//! parameter, `/<prefix>/<workspace>/<vault>/<credential>`, encrypted under
//! the customer-managed KMS key, in the Intelligent-Tiering tier, tagged as
//! Secrets Manager secrets are. Nothing is cached.

use async_trait::async_trait;
use aws_sdk_ssm::operation::delete_parameter::DeleteParameterError;
use aws_sdk_ssm::operation::get_parameter::GetParameterError;
use aws_sdk_ssm::operation::put_parameter::PutParameterError;
use aws_sdk_ssm::operation::put_parameter::builders::PutParameterFluentBuilder;
use aws_sdk_ssm::types::{ParameterTier, ParameterType, ResourceTypeForTagging, Tag};
use shared::ports::secrets::{ExposeSecret, SecretError, SecretKey, SecretStore, SecretString, into_secret};

use crate::failure::failure;
use crate::{AwsSecretsConfig, key, name};

/// Secrets as SSM parameters, under one prefix and one KMS key.
#[derive(Clone)]
pub struct AwsParameterStore {
    client: aws_sdk_ssm::Client,
    prefix: String,
    key_arn: String,
}

impl std::fmt::Debug for AwsParameterStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsParameterStore")
            .field("prefix", &self.prefix)
            .field("key_arn", &self.key_arn)
            .finish_non_exhaustive()
    }
}

impl AwsParameterStore {
    /// Check `config`'s prefix, and that its KMS key is reachable and fit to
    /// encrypt secrets, as [`crate::AwsSecretStore::connect`] does, then build
    /// the client. A parameter has no recovery window, so that setting is not read.
    pub async fn connect(config: &AwsSecretsConfig) -> Result<Self, SecretError> {
        if let Some(problem) = name::parameter_prefix_problem(&config.prefix) {
            return Err(SecretError::Backend(problem));
        }
        let (shared, key_arn) = key::connect(config).await?;
        let mut ssm = aws_sdk_ssm::config::Builder::from(&shared);
        if let Some(endpoint) = &config.endpoint {
            ssm = ssm.endpoint_url(endpoint);
        }
        Ok(Self {
            client: aws_sdk_ssm::Client::from_conf(ssm.build()),
            prefix: config.prefix.clone(),
            key_arn,
        })
    }

    /// The KMS key ARN every parameter is encrypted under.
    pub fn key_arn(&self) -> &str {
        &self.key_arn
    }

    /// The parameter name of `key`.
    pub fn parameter_name(&self, key: &SecretKey) -> String {
        name::parameter_name(&self.prefix, key)
    }

    /// `secret` written at `name`. An overwrite names the type, key and tier
    /// again: without its `KeyId`, SSM would encrypt under the AWS managed key.
    fn write(&self, name: &str, secret: &SecretString) -> PutParameterFluentBuilder {
        self.client
            .put_parameter()
            .name(name)
            .value(secret.expose_secret())
            .r#type(ParameterType::SecureString)
            .key_id(&self.key_arn)
            .tier(ParameterTier::IntelligentTiering)
    }

    /// SSM takes tags only on a create, so every overwrite is then tagged with
    /// the create's tags: a delete between the create's refusal and this write
    /// makes the overwrite a create, untagged, out of the egress policy's
    /// reach. A tagging failure keeps the value and a retry tags it again.
    async fn overwrite(&self, name: &str, secret: &SecretString, tags: Vec<Tag>) -> Result<(), SecretError> {
        let out = self.write(name, secret).overwrite(true).send().await;
        out.map_err(|e| failure("ssm:PutParameter", name, &e))?;
        let tagging = self.client.add_tags_to_resource().resource_type(ResourceTypeForTagging::Parameter);
        let tagged = tagging.resource_id(name).set_tags(Some(tags)).send().await;
        tagged.map(drop).map_err(|e| failure("ssm:AddTagsToResource", name, &e))
    }
}

#[async_trait]
impl SecretStore for AwsParameterStore {
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError> {
        let name = self.parameter_name(key);
        let tags = name::parameter_tags(key)?;
        let created = self.write(&name, secret).overwrite(false).set_tags(Some(tags.clone())).send().await;
        match created {
            Ok(_) => Ok(()),
            Err(e) if e.as_service_error().is_some_and(PutParameterError::is_parameter_already_exists) => {
                self.overwrite(&name, secret, tags).await
            }
            Err(e) => Err(failure("ssm:PutParameter", &name, &e)),
        }
    }

    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        let name = self.parameter_name(key);
        match self.client.get_parameter().name(&name).with_decryption(true).send().await {
            Ok(out) => out
                .parameter
                .and_then(|p| p.value)
                .map(into_secret)
                .ok_or_else(|| SecretError::Backend(format!("{name} holds no value"))),
            Err(e) if e.as_service_error().is_some_and(GetParameterError::is_parameter_not_found) => {
                Err(SecretError::NotFound(name))
            }
            Err(e) => Err(failure("ssm:GetParameter", &name, &e)),
        }
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        let name = self.parameter_name(key);
        match self.client.delete_parameter().name(&name).send().await {
            Ok(_) => Ok(()),
            Err(e) if e.as_service_error().is_some_and(DeleteParameterError::is_parameter_not_found) => {
                Ok(())
            }
            Err(e) => Err(failure("ssm:DeleteParameter", &name, &e)),
        }
    }
}

#[cfg(test)]
mod tests;
