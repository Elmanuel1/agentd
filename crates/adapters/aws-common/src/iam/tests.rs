use std::time::{Duration, UNIX_EPOCH};

use aws_config::{Region, SdkConfig};
use aws_credential_types::Credentials;
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_smithy_async::test_util::ManualTimeSource;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use shared::ports::credentials::{BearerTokenSource, PasswordSource};

use super::{MskAuthToken, RdsAuthToken};

const SIGNED_AT: u64 = 1_724_709_600;

fn config() -> SdkConfig {
    SdkConfig::builder()
        .credentials_provider(SharedCredentialsProvider::new(Credentials::new("akid", "secret", None, None, "test")))
        .time_source(ManualTimeSource::new(UNIX_EPOCH + Duration::from_secs(SIGNED_AT)))
        .region(Region::new("us-east-1"))
        .build()
}

/// `aws-sdk-rds`'s own `signing_works` case, byte for byte.
#[tokio::test]
async fn an_rds_token_matches_aws_generator() {
    let token = RdsAuthToken::new(config())
        .password("prod-instance.us-east-1.rds.amazonaws.com", 3306, "peccy")
        .await
        .unwrap();
    assert_eq!(
        token,
        "prod-instance.us-east-1.rds.amazonaws.com:3306/?Action=connect&DBUser=peccy&X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=akid%2F20240826%2Fus-east-1%2Frds-db%2Faws4_request&X-Amz-Date=20240826T220000Z&X-Amz-Expires=900&X-Amz-SignedHeaders=host&X-Amz-Signature=dd0cba843009474347af724090233265628ace491ea17ce3eb3da098b983ad89"
    );
}

/// The shape of `aws-msk-iam-sasl-signer-go`'s token: a base64url (unpadded)
/// presigned `kafka-cluster:Connect` URL for the region's endpoint, valid 900 s.
#[tokio::test]
async fn an_msk_token_is_a_presigned_connect_url() {
    let token = MskAuthToken::new(config()).token().await.unwrap();
    let url = String::from_utf8(URL_SAFE_NO_PAD.decode(&token.value).unwrap()).unwrap();
    let url = url::Url::parse(&url).unwrap();
    assert_eq!(url.host_str(), Some("kafka.us-east-1.amazonaws.com"));
    let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    let get = |k: &str| query.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(get("Action"), Some("kafka-cluster:Connect"));
    assert_eq!(get("X-Amz-Credential"), Some("akid/20240826/us-east-1/kafka-cluster/aws4_request"));
    assert_eq!(get("X-Amz-Date"), Some("20240826T220000Z"));
    assert_eq!(get("X-Amz-Expires"), Some("900"));
    assert_eq!(get("X-Amz-SignedHeaders"), Some("host"));
    assert!(get("X-Amz-Signature").is_some_and(|s| s.len() == 64));
    assert!(get("User-Agent").is_some_and(|ua| ua.starts_with("agentd/")));
    assert_eq!(token.expires_at_ms, i64::try_from((SIGNED_AT + 900) * 1000).unwrap());
    assert!(!token.value.contains('='), "unpadded");
}

#[tokio::test]
async fn a_token_without_credentials_is_refused() {
    let bare = SdkConfig::builder().region(Region::new("us-east-1")).build();
    let err = RdsAuthToken::new(bare).password("db", 5432, "u").await.unwrap_err();
    assert_eq!(err.to_string(), "IAM authentication needs AWS credentials");
}
