use super::TemporalConfig;

fn config(address: &str) -> TemporalConfig {
    TemporalConfig {
        address: address.into(),
        namespace: "default".into(),
        task_queue: "agentd".into(),
        api_key: None,
        server_ca: None,
    }
}

#[test]
fn http_is_plain_and_https_is_tls() {
    assert!(config("http://localhost:7233").connection().unwrap().tls_options.is_none());
    let tls = config("https://ns.acct.tmprl.cloud:7233").connection().unwrap().tls_options.unwrap();
    assert!(tls.server_root_ca_cert.is_none(), "the operating system's roots");
}

#[test]
fn the_ca_and_the_api_key_are_sent_as_given() {
    let dir = std::env::temp_dir().join(format!("agentd-temporal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ca = dir.join("ca.pem");
    std::fs::write(&ca, b"-----BEGIN CERTIFICATE-----").unwrap();
    let with = TemporalConfig {
        api_key: Some("tmprl-key".into()),
        server_ca: Some(ca.to_string_lossy().into_owned()),
        ..config("https://ns.acct.tmprl.cloud:7233")
    };
    let options = with.connection().unwrap();
    assert_eq!(options.api_key.as_deref(), Some("tmprl-key"));
    assert_eq!(
        options.tls_options.unwrap().server_root_ca_cert.as_deref(),
        Some(&b"-----BEGIN CERTIFICATE-----"[..])
    );
    assert!(!format!("{with:?}").contains("tmprl-key"));
}

#[test]
fn a_missing_ca_file_is_refused() {
    let missing =
        TemporalConfig { server_ca: Some("/nonexistent/ca.pem".into()), ..config("https://t:7233") };
    assert!(missing.connection().is_err());
}
