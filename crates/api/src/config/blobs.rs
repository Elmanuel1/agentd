//! The blob store setting: S3, the only one there is, and its timeouts.

use std::time::Duration;

use blobs_s3::S3Config;

use super::{ConfigError, endpoint, flag, number, selector, unknown};

/// `AGENTD_BLOBS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobsConfig {
    /// `s3`.
    S3(S3Config),
}

pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<BlobsConfig, ConfigError> {
    match selector(get, "AGENTD_BLOBS", "s3").as_str() {
        "s3" => Ok(BlobsConfig::S3(s3_config(get)?)),
        other => Err(unknown("AGENTD_BLOBS", other)),
    }
}

fn s3_config(get: &impl Fn(&str) -> Option<String>) -> Result<S3Config, ConfigError> {
    let endpoint = endpoint(get, "AGENTD_AWS_ENDPOINT")?;
    let ms = |var, default| number(get, var, default).map(Duration::from_millis);
    Ok(S3Config {
        bucket: get("AGENTD_BLOB_BUCKET").unwrap_or_else(|| "agentd-artifacts".into()),
        region: get("AGENTD_S3_REGION"),
        path_style: flag(get, "AGENTD_S3_PATH_STYLE", endpoint.is_some())?,
        endpoint,
        role_arn: get("AGENTD_S3_ROLE_ARN"),
        create_bucket: flag(get, "AGENTD_S3_CREATE_BUCKET", false)?,
        location_constraint: get("AGENTD_S3_LOCATION_CONSTRAINT"),
        attempt_timeout: ms("AGENTD_S3_ATTEMPT_TIMEOUT_MS", 30_000)?,
        operation_timeout: ms("AGENTD_S3_OPERATION_TIMEOUT_MS", 50_000)?,
    })
}
