use super::*;

const ENV_FIXTURE: &str = include_str!("../../../../../conformance/fixtures/environment.create.json");

#[test]
fn fixture_roundtrip_is_lossless() {
    let original: serde_json::Value = serde_json::from_str(ENV_FIXTURE).unwrap();
    let env: Environment = serde_json::from_str(ENV_FIXTURE).unwrap();
    let ours = serde_json::to_value(&env).unwrap();
    assert_eq!(ours, original);
}

#[test]
fn minimal_cloud_config_expands_to_canonical_defaults() {
    // Input from the live probe: {"type":"cloud","networking":{"type":"unrestricted"}}
    let c: EnvironmentConfig =
        serde_json::from_str(r#"{"type":"cloud","networking":{"type":"unrestricted"}}"#).unwrap();
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["type"], "cloud");
    assert_eq!(v["packages"]["type"], "packages");
    assert_eq!(v["packages"]["pip"], serde_json::json!([]));
    assert_eq!(v["networking"]["type"], "unrestricted");
    assert_eq!(v["init_script"], "");
    assert_eq!(v["environment"], serde_json::json!({}));
}

#[test]
fn self_hosted_is_bare() {
    let c: EnvironmentConfig = serde_json::from_str(r#"{"type":"self_hosted"}"#).unwrap();
    assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!({"type":"self_hosted"}));
}

#[test]
fn limited_networking_fields() {
    let n: Networking = serde_json::from_str(
        r#"{"type":"limited","allowed_hosts":["api.example.com"],"allow_package_managers":true,"allow_mcp_servers":true}"#,
    )
    .unwrap();
    let v = serde_json::to_value(&n).unwrap();
    assert_eq!(v["type"], "limited");
    assert_eq!(v["allowed_hosts"][0], "api.example.com");
    assert_eq!(v["allow_package_managers"], true);
}

fn apply(json: &str, stored: Option<&EnvironmentConfig>) -> Result<serde_json::Value, shared::ApiError> {
    let params: ConfigParams = serde_json::from_str(json).unwrap();
    params.apply(stored).map(|c| serde_json::to_value(c).unwrap())
}

#[test]
fn sdk_config_params_accept_nulls_and_untyped_packages() {
    let v =
        apply(r#"{"type":"cloud","networking":null,"packages":{"pip":["numpy"],"npm":null}}"#, None).unwrap();
    assert_eq!(v["networking"], serde_json::json!({"type":"unrestricted"}));
    assert_eq!(v["packages"]["type"], "packages");
    assert_eq!(v["packages"]["pip"], serde_json::json!(["numpy"]));
    assert_eq!(v["packages"]["npm"], serde_json::json!([]));
}

#[test]
fn limited_networking_always_serves_every_field() {
    let v = apply(r#"{"type":"cloud","networking":{"type":"limited","allowed_hosts":null}}"#, None).unwrap();
    assert_eq!(
        v["networking"],
        serde_json::json!({"type":"limited","allowed_hosts":[],"allow_package_managers":false,"allow_mcp_servers":false})
    );
}

#[test]
fn an_update_keeps_what_it_leaves_out() {
    let stored: EnvironmentConfig = serde_json::from_str(
        r#"{"type":"cloud","packages":{"type":"packages","pip":["a"]},
            "networking":{"type":"limited","allowed_hosts":["h"],"allow_package_managers":true}}"#,
    )
    .unwrap();
    let v =
        apply(r#"{"type":"cloud","networking":{"type":"limited","allow_mcp_servers":true}}"#, Some(&stored))
            .unwrap();
    assert_eq!(v["packages"]["pip"], serde_json::json!(["a"]));
    assert_eq!(
        v["networking"],
        serde_json::json!({"type":"limited","allowed_hosts":["h"],"allow_package_managers":true,"allow_mcp_servers":true})
    );
}

#[test]
fn packages_need_package_managers_under_limited_networking() {
    let err = apply(r#"{"type":"cloud","networking":{"type":"limited"},"packages":{"pip":["a"]}}"#, None)
        .unwrap_err();
    assert!(err.message.contains("allow_package_managers"), "{}", err.message);
}

#[test]
fn create_takes_the_sdk_scope_and_description_forms() {
    let r: CreateEnvironmentRequest =
        serde_json::from_str(r#"{"name":"e","scope":"account","description":null}"#).unwrap();
    assert_eq!(r.scope, Some(ScopeParam::Account));
    assert_eq!(r.description, shared::Patch::Clear);
}
