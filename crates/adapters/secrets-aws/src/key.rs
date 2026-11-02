//! The KMS key check at connect.

use aws_sdk_kms::types::{KeyManagerType, KeyMetadata, KeySpec, KeyState, KeyUsageType};
use shared::ports::secrets::SecretError;

use crate::failure::failure;

/// The ARN of `key_id` once `DescribeKey` shows it is a customer-managed,
/// enabled, symmetric encryption key. Secrets are created under the ARN, so a
/// policy condition on `secretsmanager:KmsKeyArn` matches whatever form of id
/// was configured.
pub(crate) async fn customer_key_arn(kms: &aws_sdk_kms::Client, key_id: &str) -> Result<String, SecretError> {
    if key_id.trim().is_empty() {
        return Err(SecretError::Backend("a customer-managed KMS key id is required".into()));
    }
    let out =
        kms.describe_key().key_id(key_id).send().await.map_err(|e| failure("kms:DescribeKey", key_id, &e))?;
    let meta = out.key_metadata.ok_or_else(|| unfit(key_id, "DescribeKey returned no key metadata"))?;
    check(key_id, &meta)
}

fn check(key_id: &str, meta: &KeyMetadata) -> Result<String, SecretError> {
    if meta.key_manager() != Some(&KeyManagerType::Customer) {
        return Err(unfit(key_id, "it is not customer managed (the aws/secretsmanager key is not accepted)"));
    }
    if !meta.enabled() || meta.key_state() != Some(&KeyState::Enabled) {
        return Err(unfit(key_id, "it is not enabled"));
    }
    if meta.key_usage() != Some(&KeyUsageType::EncryptDecrypt)
        || meta.key_spec() != Some(&KeySpec::SymmetricDefault)
    {
        return Err(unfit(key_id, "it is not a symmetric encryption key"));
    }
    meta.arn().map(str::to_owned).ok_or_else(|| unfit(key_id, "DescribeKey returned no key ARN"))
}

fn unfit(key_id: &str, why: &str) -> SecretError {
    SecretError::Backend(format!("KMS key {key_id} cannot encrypt secrets: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> aws_sdk_kms::types::builders::KeyMetadataBuilder {
        KeyMetadata::builder()
            .key_id("k")
            .arn("arn:aws:kms:us-east-1:111122223333:key/k")
            .key_manager(KeyManagerType::Customer)
            .enabled(true)
            .key_state(KeyState::Enabled)
            .key_usage(KeyUsageType::EncryptDecrypt)
            .key_spec(KeySpec::SymmetricDefault)
    }

    #[test]
    fn a_customer_symmetric_enabled_key_gives_its_arn() {
        assert_eq!(check("k", &key().build().unwrap()).unwrap(), "arn:aws:kms:us-east-1:111122223333:key/k");
    }

    #[test]
    fn any_other_key_is_refused() {
        let cases = [
            (key().key_manager(KeyManagerType::Aws), "customer managed"),
            (key().enabled(false), "not enabled"),
            (key().key_state(KeyState::PendingDeletion), "not enabled"),
            (key().key_usage(KeyUsageType::SignVerify), "symmetric"),
            (key().key_spec(KeySpec::Rsa2048), "symmetric"),
            (key().set_arn(None), "no key ARN"),
        ];
        for (meta, why) in cases {
            let err = check("k", &meta.build().unwrap()).unwrap_err().to_string();
            assert!(err.contains(why), "{err}");
        }
    }
}
