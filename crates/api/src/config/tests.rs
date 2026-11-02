//! Config parsing tests.

use std::collections::HashMap;

use secrets_aws::AwsSecretsConfig;

use super::*;

/// `vars` over a KMS key id, which the default secret store needs.
fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
    let mut all: HashMap<String, String> =
        HashMap::from([("AGENTD_SECRETS_KMS_KEY_ID".into(), "alias/agentd".into())]);
    all.extend(vars.iter().map(|(k, v)| ((*k).into(), (*v).into())));
    Config::from_lookup(|k| all.get(k).cloned())
}

#[test]
fn defaults_select_postgres_s3_aws_env_local_none() {
    let c = config(&[("AGENTD_API_KEYS", "k:org_a:wrkspc_a")]).unwrap();
    assert!(matches!(c.store, StoreConfig::Postgres { .. }));
    assert_eq!(c.blobs, BlobsConfig::S3(S3Config::new("agentd-artifacts")));
    assert_eq!(c.secrets, SecretsConfig::Aws(AwsSecretsConfig::new("agentd", "alias/agentd")));
    assert_eq!((c.scheduler, c.runtime), (SchedulerConfig::Local, RuntimeConfig::None));
    let KeysConfig::Env(keys) = c.keys;
    assert_eq!(keys.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), ["k"]);
}

#[test]
fn an_unknown_backend_is_an_error() {
    let err = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_STORE", "dynamodb")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_STORE");
}

#[test]
fn no_key_is_registered_without_the_dev_flag() {
    assert_eq!(config(&[]).unwrap_err().var, "AGENTD_API_KEYS");
    let c = config(&[("AGENTD_DEV_KEY", "1")]).unwrap();
    let KeysConfig::Env(keys) = c.keys;
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key, "agentd-dev-key");
}

#[test]
fn empty_key_entries_are_skipped() {
    let c = config(&[("AGENTD_API_KEYS", ""), ("AGENTD_DEV_KEY", "1")]).unwrap();
    let KeysConfig::Env(keys) = c.keys;
    assert_eq!(keys.len(), 1);
    let c = config(&[("AGENTD_API_KEYS", "k:o:w,")]).unwrap();
    let KeysConfig::Env(keys) = c.keys;
    assert_eq!(keys.len(), 1);
}

#[test]
fn debug_output_omits_the_key() {
    let c = config(&[("AGENTD_API_KEYS", "s3cr3t-key:o:w")]).unwrap();
    assert!(!format!("{c:?}").contains("s3cr3t-key"));
}

#[test]
fn a_malformed_key_entry_is_an_error() {
    let err = config(&[("AGENTD_API_KEYS", "k:o:w,broken")]).unwrap_err();
    assert_eq!(
        err,
        ConfigError {
            var: "AGENTD_API_KEYS",
            reason: "entry 1 is not key:org:workspace, with a workspace id of letters, digits and _".into(),
        }
    );
}

#[test]
fn s3_options_come_from_config() {
    let c = config(&[
        ("AGENTD_API_KEYS", "k:o:w"),
        ("AGENTD_BLOB_BUCKET", "b"),
        ("AGENTD_S3_REGION", "eu-west-1"),
        ("AGENTD_AWS_ENDPOINT", "http://localhost:4566"),
        ("AGENTD_S3_ROLE_ARN", "arn:aws:iam::123456789012:role/agentd"),
        ("AGENTD_S3_CREATE_BUCKET", "true"),
        ("AGENTD_S3_LOCATION_CONSTRAINT", "eu-west-1-standard"),
    ])
    .unwrap();
    assert_eq!(
        c.blobs,
        BlobsConfig::S3(S3Config {
            bucket: "b".into(),
            region: Some("eu-west-1".into()),
            endpoint: Some("http://localhost:4566".into()),
            path_style: true,
            role_arn: Some("arn:aws:iam::123456789012:role/agentd".into()),
            create_bucket: true,
            location_constraint: Some("eu-west-1-standard".into()),
        })
    );
    let c = config(&[
        ("AGENTD_API_KEYS", "k:o:w"),
        ("AGENTD_AWS_ENDPOINT", "http://m"),
        ("AGENTD_S3_PATH_STYLE", "0"),
    ])
    .unwrap();
    assert!(matches!(c.blobs, BlobsConfig::S3(S3Config { path_style: false, .. })));
    assert_eq!(
        config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_S3_CREATE_BUCKET", "yes")]).unwrap_err().var,
        "AGENTD_S3_CREATE_BUCKET"
    );
}

#[test]
fn telemetry_is_off_and_logs_are_text_unless_configured() {
    let c = config(&[("AGENTD_API_KEYS", "k:o:w")]).unwrap();
    assert_eq!((c.telemetry.otlp, c.telemetry.json_logs), (false, false));
    let c = config(&[
        ("AGENTD_API_KEYS", "k:o:w"),
        ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"),
        ("AGENTD_LOG_FORMAT", "json"),
    ])
    .unwrap();
    assert_eq!((c.telemetry.otlp, c.telemetry.json_logs), (true, true));
    let err = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_LOG_FORMAT", "xml")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_LOG_FORMAT");
}

#[test]
fn the_pool_is_sized_by_config_and_rejects_nonsense() {
    let c = config(&[
        ("AGENTD_API_KEYS", "k:o:w"),
        ("AGENTD_DB_POOL_SIZE", "32"),
        ("AGENTD_DB_POOL_WAIT_MS", "250"),
    ])
    .unwrap();
    let StoreConfig::Postgres { pool, .. } = c.store;
    assert_eq!((pool.max_size, pool.wait), (32, std::time::Duration::from_millis(250)));
    let err = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_DB_POOL_SIZE", "0")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_DB_POOL_SIZE");
}

#[test]
fn the_aws_secret_store_needs_a_kms_key() {
    for missing in ["", "  "] {
        let err =
            config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_SECRETS_KMS_KEY_ID", missing)]).unwrap_err();
        assert_eq!(err.var, "AGENTD_SECRETS_KMS_KEY_ID");
    }
}

#[test]
fn aws_secret_options_come_from_config() {
    let c = config(&[
        ("AGENTD_API_KEYS", "k:o:w"),
        ("AGENTD_SECRETS", "aws"),
        ("AGENTD_SECRETS_KMS_KEY_ID", "arn:aws:kms:eu-west-1:111122223333:key/k"),
        ("AGENTD_SECRETS_PREFIX", "agentd/prod"),
        ("AGENTD_SECRETS_REGION", "eu-west-1"),
        ("AGENTD_SECRETS_ENDPOINT", "https://secretsmanager.eu-west-1.amazonaws.com"),
        ("AGENTD_SECRETS_ROLE_ARN", "arn:aws:iam::111122223333:role/agentd-secrets"),
        ("AGENTD_SECRETS_RECOVERY_DAYS", "30"),
    ])
    .unwrap();
    assert_eq!(
        c.secrets,
        SecretsConfig::Aws(AwsSecretsConfig {
            prefix: "agentd/prod".into(),
            kms_key_id: "arn:aws:kms:eu-west-1:111122223333:key/k".into(),
            region: Some("eu-west-1".into()),
            endpoint: Some("https://secretsmanager.eu-west-1.amazonaws.com".into()),
            role_arn: Some("arn:aws:iam::111122223333:role/agentd-secrets".into()),
            recovery_window_days: 30,
        })
    );
    let err = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_SECRETS_RECOVERY_DAYS", "-1")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_SECRETS_RECOVERY_DAYS");
}

#[test]
fn the_unencrypted_blob_secret_store_needs_the_dev_flag() {
    let err = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_SECRETS", "blob")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_SECRETS");
    assert!(err.reason.contains("AGENTD_DEV_SECRETS=1"), "{}", err.reason);
    let err =
        config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_SECRETS", "blob"), ("AGENTD_DEV_SECRETS", "0")])
            .unwrap_err();
    assert_eq!(err.var, "AGENTD_SECRETS");
    let c = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_SECRETS", "blob"), ("AGENTD_DEV_SECRETS", "1")])
        .unwrap();
    assert_eq!(c.secrets, SecretsConfig::Blob);
    let err = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_SECRETS", "vault")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_SECRETS");
}

#[test]
fn a_plain_http_secrets_endpoint_needs_the_dev_flag() {
    let local = [("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_SECRETS_ENDPOINT", "http://localhost:4566")];
    assert_eq!(config(&local).unwrap_err().var, "AGENTD_SECRETS_ENDPOINT");
    let c = config(&[local[0], local[1], ("AGENTD_DEV_SECRETS", "1")]).unwrap();
    assert!(matches!(c.secrets, SecretsConfig::Aws(AwsSecretsConfig { endpoint: Some(_), .. })));
}

#[test]
fn a_workspace_id_outside_the_id_charset_is_an_error() {
    for workspace in ["wrkspc/../x", "wrkspc-a", "wrkspc.a", ""] {
        let keys = format!("k:o:{workspace}");
        assert_eq!(
            config(&[("AGENTD_API_KEYS", &keys)]).unwrap_err().var,
            "AGENTD_API_KEYS",
            "{workspace:?}"
        );
    }
}
