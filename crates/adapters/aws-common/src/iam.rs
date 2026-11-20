//! IAM authentication tokens, signed with SigV4 from the shared
//! configuration's credentials: an RDS token is a Postgres password, an MSK
//! token a Kafka OAUTHBEARER token. Both are presigned URLs valid 15 minutes,
//! built as AWS's own signers build them (`aws-sdk-rds`'s
//! `AuthTokenGenerator`, `aws-msk-iam-sasl-signer-go`).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_config::SdkConfig;
use aws_credential_types::provider::ProvideCredentials;
use aws_sigv4::http_request::{self, SignableBody, SignableRequest, SignatureLocation, SigningSettings};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use shared::BackendError;
use shared::ports::BoxFuture;
use shared::ports::credentials::{BearerToken, BearerTokenSource, PasswordSource};

const EXPIRES_IN: Duration = Duration::from_secs(900);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct IamError(String);

fn failed(e: impl std::fmt::Display) -> BackendError {
    BackendError::new(IamError(e.to_string()))
}

/// RDS IAM database authentication: each login's password is a token for
/// that host, port and user (<https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/UsingWithRDS.IAMDBAuth.html>).
#[derive(Debug, Clone)]
pub struct RdsAuthToken(SdkConfig);

impl RdsAuthToken {
    /// Tokens signed as `config`'s credentials, in its region.
    pub fn new(config: SdkConfig) -> Self {
        Self(config)
    }
}

impl PasswordSource for RdsAuthToken {
    fn password<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        user: &'a str,
    ) -> BoxFuture<'a, Result<String, BackendError>> {
        Box::pin(async move {
            let url = format!("https://{host}:{port}/?Action=connect&DBUser={user}");
            let (signed, _) = presign(&self.0, &url, "rds-db").await?;
            Ok(signed.as_str().trim_start_matches("https://").to_owned())
        })
    }
}

/// Amazon MSK IAM access control over SASL/OAUTHBEARER
/// (<https://docs.aws.amazon.com/msk/latest/developerguide/iam-access-control.html>).
#[derive(Debug, Clone)]
pub struct MskAuthToken(SdkConfig);

impl MskAuthToken {
    /// Tokens signed as `config`'s credentials, for the brokers in its region.
    pub fn new(config: SdkConfig) -> Self {
        Self(config)
    }
}

impl BearerTokenSource for MskAuthToken {
    fn token(&self) -> BoxFuture<'_, Result<BearerToken, BackendError>> {
        Box::pin(async move {
            let region = self.0.region().map_or("us-east-1", |r| r.as_ref());
            let url = format!("https://kafka.{region}.amazonaws.com/?Action=kafka-cluster%3AConnect");
            let (mut signed, now) = presign(&self.0, &url, "kafka-cluster").await?;
            signed.query_pairs_mut().append_pair("User-Agent", concat!("agentd/", env!("CARGO_PKG_VERSION")));
            let expires_at = now + EXPIRES_IN;
            let expires_at_ms = expires_at.duration_since(UNIX_EPOCH).map_err(failed)?.as_millis();
            Ok(BearerToken {
                value: URL_SAFE_NO_PAD.encode(signed.as_str()),
                principal: "agentd".into(),
                expires_at_ms: i64::try_from(expires_at_ms).map_err(failed)?,
            })
        })
    }
}

/// `url` presigned for `service` (the signature and its parameters in the
/// query), and the time it was signed at, from the configuration's clock.
async fn presign(
    config: &SdkConfig,
    url: &str,
    service: &str,
) -> Result<(url::Url, SystemTime), BackendError> {
    let now = config.time_source().map_or_else(SystemTime::now, |t| t.now());
    let credentials = config
        .credentials_provider()
        .ok_or_else(|| failed("IAM authentication needs AWS credentials"))?
        .provide_credentials()
        .await
        .map_err(failed)?;
    let identity: Identity = credentials.into();
    let region = config.region().map_or("us-east-1", |r| r.as_ref());
    let mut settings = SigningSettings::default();
    settings.expires_in = Some(EXPIRES_IN);
    settings.signature_location = SignatureLocation::QueryParams;
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(service)
        .time(now)
        .settings(settings)
        .build()
        .map_err(failed)?;
    let request =
        SignableRequest::new("GET", url, std::iter::empty(), SignableBody::Bytes(&[])).map_err(failed)?;
    let (instructions, _) = http_request::sign(request, &params.into()).map_err(failed)?.into_parts();
    let mut signed = url::Url::parse(url).map_err(failed)?;
    for (name, value) in instructions.params() {
        signed.query_pairs_mut().append_pair(name, value);
    }
    Ok((signed, now))
}

#[cfg(test)]
mod tests;
