use super::*;

const ENV_FIXTURE: &str = include_str!("../../../../fixtures/environment.create.json");

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
