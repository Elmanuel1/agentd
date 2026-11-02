//! aws-common: the AWS configuration every AWS adapter starts from.
//!
//! Credentials always come from the AWS default chain (env keys, profile, web
//! identity, ECS task or EC2 instance role), optionally exchanged for an
//! assumed role. Each adapter applies its own endpoint to its own client, so
//! STS keeps the real one.

#![forbid(unsafe_code)]

use aws_config::environment::EnvironmentVariableRegionProvider;
use aws_config::meta::region::RegionProviderChain;
use aws_config::profile::ProfileFileRegionProvider;
use aws_config::sts::AssumeRoleProvider;
use aws_config::{BehaviorVersion, Region, SdkConfig};
use aws_credential_types::provider::SharedCredentialsProvider;

/// The shared configuration in `region` (`None` takes `AWS_REGION`,
/// `AWS_DEFAULT_REGION` or the profile's, else `us-east-1`), calling as
/// `role_arn` when one is given.
pub async fn sdk_config(region: Option<&str>, role_arn: Option<&str>) -> SdkConfig {
    // The region chain skips IMDS so a boot off EC2 never waits on it;
    // ECS and Lambda set AWS_REGION.
    let region = RegionProviderChain::first_try(region.map(|r| Region::new(r.to_owned())))
        .or_else(EnvironmentVariableRegionProvider::new())
        .or_else(ProfileFileRegionProvider::new())
        .or_else(Region::from_static("us-east-1"));
    let shared = aws_config::defaults(BehaviorVersion::latest()).region(region).load().await;
    match role_arn {
        None => shared,
        Some(role_arn) => {
            let role =
                AssumeRoleProvider::builder(role_arn).session_name("agentd").configure(&shared).build().await;
            shared.into_builder().credentials_provider(SharedCredentialsProvider::new(role)).build()
        }
    }
}
