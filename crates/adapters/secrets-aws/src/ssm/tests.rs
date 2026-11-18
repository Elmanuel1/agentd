//! `AwsParameterStore`'s writes, and the overwrite that creates a parameter
//! (against LocalStack, when `AGENTD_AWS_ENDPOINT` names it).

use aws_sdk_ssm::config::{BehaviorVersion, Region};

use super::*;

fn store(client: aws_sdk_ssm::Client, key_arn: &str) -> AwsParameterStore {
    AwsParameterStore { client, prefix: "agentd_test/ssm".into(), key_arn: key_arn.into() }
}

#[test]
fn a_write_is_a_secure_string_under_the_key_in_intelligent_tiering() {
    let conf = aws_sdk_ssm::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .build();
    let store = store(aws_sdk_ssm::Client::from_conf(conf), "arn:aws:kms:us-east-1:111122223333:key/k");
    let write = store.write("/agentd/w/v/c", &SecretString::from("v"));
    let input = write.as_input();
    assert_eq!(input.get_type(), &Some(ParameterType::SecureString));
    assert_eq!(input.get_key_id().as_deref(), Some(store.key_arn()));
    assert_eq!(input.get_tier(), &Some(ParameterTier::IntelligentTiering));
}

/// The race a test cannot time: a delete between the create's refusal and
/// the overwrite, which then creates the parameter. Runs against LocalStack.
#[tokio::test]
async fn an_overwrite_that_creates_the_parameter_tags_it() {
    let Ok(endpoint) = std::env::var("AGENTD_AWS_ENDPOINT") else { return };
    let shared = aws_common::sdk_config(None, None).await;
    let conf = aws_sdk_ssm::config::Builder::from(&shared).endpoint_url(&endpoint).build();
    let store = store(aws_sdk_ssm::Client::from_conf(conf), "alias/aws/ssm");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let key = SecretKey::new(&format!("wrkspc_{id}"), "vlt_01a", "cred_01b").unwrap().environment_variable();
    let name = store.parameter_name(&key);
    let tags = name::parameter_tags(&key).unwrap();
    for value in ["first", "second"] {
        store.overwrite(&name, &SecretString::from(value), tags.clone()).await.unwrap();
    }
    let listed = store.client.list_tags_for_resource().resource_type(ResourceTypeForTagging::Parameter);
    let mut found = listed.resource_id(&name).send().await.unwrap().tag_list.unwrap_or_default();
    found.sort_by(|a, b| a.key.cmp(&b.key));
    let mut want = tags;
    want.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(found, want);
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "second");
    store.delete(&key).await.unwrap();
}

/// Refuses every `AddTagsToResource` before it is sent.
#[derive(Debug)]
struct NoTagging;

impl aws_smithy_runtime_api::client::interceptors::Intercept for NoTagging {
    fn name(&self) -> &'static str {
        "NoTagging"
    }

    fn read_before_transmit(
        &self,
        context: &aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextRef<
            '_,
        >,
        _: &aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
        _: &mut aws_smithy_types::config_bag::ConfigBag,
    ) -> Result<(), aws_smithy_runtime_api::box_error::BoxError> {
        match context.request().headers().get("x-amz-target") {
            Some("AmazonSSM.AddTagsToResource") => Err("tagging refused".into()),
            _ => Ok(()),
        }
    }
}

#[tokio::test]
async fn a_parameter_the_overwrite_created_goes_when_tagging_fails() {
    let Ok(endpoint) = std::env::var("AGENTD_AWS_ENDPOINT") else { return };
    let shared = aws_common::sdk_config(None, None).await;
    let conf = aws_sdk_ssm::config::Builder::from(&shared).endpoint_url(&endpoint).interceptor(NoTagging);
    let store = store(aws_sdk_ssm::Client::from_conf(conf.build()), "alias/aws/ssm");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let key = SecretKey::new(&format!("wrkspc_{id}"), "vlt_01a", "cred_01b").unwrap().environment_variable();
    let name = store.parameter_name(&key);
    let tags = name::parameter_tags(&key).unwrap();
    let err = store.overwrite(&name, &SecretString::from("v"), tags).await.unwrap_err();
    assert!(err.to_string().contains("ssm:AddTagsToResource"), "{err}");
    assert!(matches!(store.get(&key).await, Err(SecretError::NotFound(_))), "a retry creates it, tagged");
}
