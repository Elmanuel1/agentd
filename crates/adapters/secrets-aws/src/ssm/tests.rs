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

/// Refuses each call whose target it names, before the call is sent.
#[derive(Debug)]
struct Refuse(&'static [&'static str]);

impl aws_smithy_runtime_api::client::interceptors::Intercept for Refuse {
    fn name(&self) -> &'static str {
        "Refuse"
    }

    fn read_before_transmit(
        &self,
        context: &aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextRef<
            '_,
        >,
        _: &aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
        _: &mut aws_smithy_types::config_bag::ConfigBag,
    ) -> Result<(), aws_smithy_runtime_api::box_error::BoxError> {
        let target = context.request().headers().get("x-amz-target").unwrap_or_default();
        match self.0.iter().any(|refused| target == format!("AmazonSSM.{refused}")) {
            true => Err(format!("{target} refused").into()),
            false => Ok(()),
        }
    }
}

/// A store on LocalStack that refuses the `refused` calls, and a fresh
/// `environment_variable` key; `None` without `AGENTD_AWS_ENDPOINT`.
async fn on_localstack(refused: &'static [&'static str]) -> Option<(AwsParameterStore, SecretKey)> {
    let endpoint = std::env::var("AGENTD_AWS_ENDPOINT").ok()?;
    let shared = aws_common::sdk_config(None, None).await;
    let conf =
        aws_sdk_ssm::config::Builder::from(&shared).endpoint_url(&endpoint).interceptor(Refuse(refused));
    let store = store(aws_sdk_ssm::Client::from_conf(conf.build()), "alias/aws/ssm");
    let id = uuid::Uuid::new_v4().simple().to_string();
    let key = SecretKey::new(&format!("wrkspc_{id}"), "vlt_01a", "cred_01b").unwrap().environment_variable();
    Some((store, key))
}

/// The race a test cannot time: a delete between the create's refusal and
/// the overwrite, which then creates the parameter.
#[tokio::test]
async fn an_overwrite_that_creates_the_parameter_tags_it() {
    let Some((store, key)) = on_localstack(&[]).await else { return };
    let tags = name::parameter_tags(&key).unwrap();
    for value in ["first", "second"] {
        store.overwrite(&key, &SecretString::from(value), tags.clone()).await.unwrap();
    }
    let listed = store.client.list_tags_for_resource().resource_type(ResourceTypeForTagging::Parameter);
    let name = store.parameter_name(&key);
    let mut found = listed.resource_id(&name).send().await.unwrap().tag_list.unwrap_or_default();
    found.sort_by(|a, b| a.key.cmp(&b.key));
    let mut want = tags;
    want.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(found, want);
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "second");
    store.delete(&key).await.unwrap();
}

#[tokio::test]
async fn a_parameter_the_overwrite_created_goes_when_tagging_fails() {
    let Some((store, key)) = on_localstack(&["AddTagsToResource"]).await else { return };
    let tags = name::parameter_tags(&key).unwrap();
    let err = store.overwrite(&key, &SecretString::from("v"), tags).await.unwrap_err();
    assert!(err.to_string().contains("ssm:AddTagsToResource"), "{err}");
    assert!(matches!(store.get(&key).await, Err(SecretError::NotFound(_))), "a retry creates it, tagged");
}

#[tokio::test]
async fn a_delete_that_fails_after_tagging_fails_is_the_error() {
    let Some((store, key)) = on_localstack(&["AddTagsToResource", "DeleteParameter"]).await else { return };
    let tags = name::parameter_tags(&key).unwrap();
    let err = store.overwrite(&key, &SecretString::from("v"), tags).await.unwrap_err();
    assert!(err.to_string().contains("ssm:DeleteParameter"), "{err}");
    let (plain, _) = on_localstack(&[]).await.unwrap();
    plain.delete(&key).await.unwrap();
}
