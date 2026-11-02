//! [`SecretStore`] on Secrets Manager.

use async_trait::async_trait;
use aws_sdk_secretsmanager::operation::create_secret::CreateSecretError;
use aws_sdk_secretsmanager::operation::delete_secret::DeleteSecretError;
use aws_sdk_secretsmanager::operation::describe_secret::DescribeSecretError;
use aws_sdk_secretsmanager::operation::get_secret_value::GetSecretValueError;
use shared::ports::secrets::{ExposeSecret, SecretError, SecretKey, SecretStore, SecretString};
use zeroize::Zeroize;

use crate::failure::failure;
use crate::{name, AwsSecretStore};

impl AwsSecretStore {
    /// Whether `name` exists and is scheduled for deletion: Secrets Manager
    /// answers a call on such a secret with `InvalidRequestException`, which
    /// other faults share.
    async fn scheduled_for_deletion(&self, name: &str) -> Result<bool, SecretError> {
        match self.client.describe_secret().secret_id(name).send().await {
            Ok(out) => Ok(out.deleted_date.is_some()),
            Err(e)
                if e.as_service_error().is_some_and(DescribeSecretError::is_resource_not_found_exception) =>
            {
                Ok(false)
            }
            Err(e) => Err(failure("secretsmanager:DescribeSecret", name, &e)),
        }
    }

    /// The current value of the secret `name`; `NotFound` when it is missing
    /// or scheduled for deletion.
    pub(crate) async fn read(&self, name: &str) -> Result<SecretString, SecretError> {
        let e = match self.client.get_secret_value().secret_id(name).send().await {
            Ok(out) => {
                return out
                    .secret_string
                    .map(into_secret)
                    .ok_or_else(|| SecretError::Backend(format!("{name} holds no string value")));
            }
            Err(e) => e,
        };
        let service = e.as_service_error();
        if service.is_some_and(GetSecretValueError::is_resource_not_found_exception)
            || (service.is_some_and(GetSecretValueError::is_invalid_request_exception)
                && self.scheduled_for_deletion(name).await?)
        {
            return Err(SecretError::NotFound(name.to_owned()));
        }
        Err(failure("secretsmanager:GetSecretValue", name, &e))
    }
}

#[async_trait]
impl SecretStore for AwsSecretStore {
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError> {
        let name = self.secret_name(key);
        // One token for the write, whichever call makes it: a retry with it
        // is a no-op, never a second version.
        let token = uuid::Uuid::new_v4().to_string();
        let created = self
            .client
            .create_secret()
            .name(&name)
            .kms_key_id(&self.key_arn)
            .secret_string(secret.expose_secret())
            .client_request_token(&token)
            .set_tags(Some(name::tags(key)))
            .send()
            .await;
        let e = match created {
            Ok(_) => return Ok(()),
            Err(e) => e,
        };
        if e.as_service_error().is_some_and(CreateSecretError::is_invalid_request_exception)
            && self.scheduled_for_deletion(&name).await?
        {
            // Deleted inside its recovery window: restore it, then the new
            // value becomes its current version.
            self.client
                .restore_secret()
                .secret_id(&name)
                .send()
                .await
                .map_err(|e| failure("secretsmanager:RestoreSecret", &name, &e))?;
        } else if !e.as_service_error().is_some_and(CreateSecretError::is_resource_exists_exception) {
            return Err(failure("secretsmanager:CreateSecret", &name, &e));
        }
        self.client
            .put_secret_value()
            .secret_id(&name)
            .secret_string(secret.expose_secret())
            .client_request_token(&token)
            .send()
            .await
            .map_err(|e| failure("secretsmanager:PutSecretValue", &name, &e))?;
        Ok(())
    }

    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        self.read(&self.secret_name(key)).await
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        let name = self.secret_name(key);
        let deleted = self
            .client
            .delete_secret()
            .secret_id(&name)
            .recovery_window_in_days(self.recovery_window_days)
            .send()
            .await;
        let e = match deleted {
            Ok(_) => return Ok(()),
            Err(e) => e,
        };
        let service = e.as_service_error();
        if service.is_some_and(DeleteSecretError::is_resource_not_found_exception)
            || (service.is_some_and(DeleteSecretError::is_invalid_request_exception)
                && self.scheduled_for_deletion(&name).await?)
        {
            return Ok(());
        }
        Err(failure("secretsmanager:DeleteSecret", &name, &e))
    }
}

/// The SDK's string can have spare capacity, and turning it into a
/// [`SecretString`] would then reallocate and free the original unzeroed: copy
/// it once at its exact size and zero the original.
pub(crate) fn into_secret(mut value: String) -> SecretString {
    let secret = SecretString::from(value.as_str());
    value.zeroize();
    secret
}
