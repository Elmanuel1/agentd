//! aws-common: the AWS configuration every AWS adapter starts from.
//!
//! Credentials always come from the AWS default chain (env keys, profile, web
//! identity, ECS task or EC2 instance role), optionally exchanged for an
//! assumed role. Each adapter applies its own endpoint to its own client, so
//! STS keeps the real one.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

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

/// Why `endpoint` may not be used, if it may not: an endpoint override is
/// `https://` to any host, or `http://` only to the machine itself (a loopback
/// address, `localhost`) or a single-label host name (a container or service
/// on a private network, as an emulator runs on), which no public DNS name
/// is. Plain HTTP would carry credentials and request signatures in clear.
pub fn endpoint_problem(endpoint: &str) -> Option<&'static str> {
    if let Some(rest) = endpoint.strip_prefix("https://") {
        return host(rest).is_empty().then_some("has no host");
    }
    let Some(rest) = endpoint.strip_prefix("http://") else {
        return Some("is neither https:// nor http://");
    };
    let host = host(rest);
    let private = host == "localhost"
        || host.ends_with(".localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
        || (!host.is_empty() && !host.contains('.') && !host.contains(':'));
    (!private).then_some(
        "is plain http to a host that is not loopback or a single-label container name; use https://",
    )
}

/// The host of `rest`, the part of a URL after its scheme: up to the port or
/// path, without IPv6 brackets.
fn host(rest: &str) -> &str {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit('@').next().unwrap_or_default();
    match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_is_tls_or_stays_on_a_private_name() {
        for ok in [
            "https://secretsmanager.eu-west-1.amazonaws.com",
            "https://vpce-1.secretsmanager.eu-west-1.vpce.amazonaws.com:443/",
            "http://localhost:4566",
            "http://127.0.0.1:4566",
            "http://[::1]:4566/",
            "http://localstack:4566",
            "http://s3.localhost:4566",
        ] {
            assert_eq!(endpoint_problem(ok), None, "{ok}");
        }
        for bad in [
            "http://secretsmanager.eu-west-1.amazonaws.com",
            "http://10.0.0.5:4566",
            "http://[2001:db8::1]:4566",
            "http://evil.example.com",
            "http://localhost.evil.com",
            "http://user@evil.example.com",
            "http://:4566",
            "https://",
            "ftp://localstack",
            "localstack:4566",
        ] {
            assert!(endpoint_problem(bad).is_some(), "{bad}");
        }
    }
}
