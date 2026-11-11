//! Config parsing tests.

use std::collections::HashMap;

use secrets_aws::AwsSecretsConfig;

use super::*;

const URL: &str = "postgres://agentd:s3cr3t-pass@db:5432/agentd";

/// `vars` over a database URL and a KMS key id, which every boot needs.
fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
    let mut all: HashMap<String, String> = HashMap::from([
        ("DATABASE_URL".into(), URL.into()),
        ("AGENTD_SECRETS_KMS_KEY_ID".into(), "alias/agentd".into()),
    ]);
    all.extend(vars.iter().map(|(k, v)| ((*k).into(), (*v).into())));
    Config::from_lookup(|k| all.get(k).cloned())
}

#[test]
fn defaults_select_postgres_s3_aws_aws_keys_local_docker() {
    let c = config(&[]).unwrap();
    let StoreConfig::Postgres { url, .. } = &c.store;
    assert_eq!(url, URL);
    assert_eq!(c.blobs, BlobsConfig::S3(S3Config::new("agentd-artifacts")));
    assert_eq!(c.secrets, AwsSecretsConfig::new("agentd", "alias/agentd"));
    assert_eq!((c.keys, c.scheduler), (KeysConfig::Aws, SchedulerConfig::Local));
    assert!(matches!(c.runtime, RuntimeConfig::Docker { ref image, .. } if image == DEFAULT_SANDBOX_IMAGE));
}

#[test]
fn an_unknown_backend_is_an_error() {
    let err = config(&[("AGENTD_STORE", "dynamodb")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_STORE");
}

#[test]
fn no_value_weakens_security() {
    for (var, value) in [("AGENTD_SECRETS", "blob"), ("AGENTD_KEYS", "env"), ("AGENTD_RUNTIME", "none")] {
        assert_eq!(config(&[(var, value)]).unwrap_err().var, var, "{var}={value}");
    }
    // The removed development switches are not read at all.
    let c = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_DEV_KEY", "1"), ("AGENTD_DEV_SECRETS", "1")])
        .unwrap();
    assert_eq!(c, config(&[]).unwrap());
}

#[test]
fn the_database_url_is_required() {
    assert_eq!(config(&[("DATABASE_URL", "")]).unwrap_err().var, "DATABASE_URL");
}

#[test]
fn debug_output_omits_the_database_password() {
    let shown = format!("{:?}", config(&[]).unwrap());
    assert!(!shown.contains("s3cr3t-pass"), "{shown}");
    assert!(shown.contains("db:5432/agentd"), "{shown}");
    for url in ["postgres://agentd@db/agentd?password=s3cr3t-pass", "postgres://a:s3cr3t-pass@x@db/agentd"] {
        let shown = format!("{:?}", config(&[("DATABASE_URL", url)]).unwrap());
        assert!(!shown.contains("s3cr3t-pass"), "{shown}");
    }
}

#[test]
fn s3_options_come_from_config() {
    let c = config(&[
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
    let c = config(&[("AGENTD_AWS_ENDPOINT", "http://m"), ("AGENTD_S3_PATH_STYLE", "0")]).unwrap();
    assert!(matches!(c.blobs, BlobsConfig::S3(S3Config { path_style: false, .. })));
    assert_eq!(config(&[("AGENTD_S3_CREATE_BUCKET", "yes")]).unwrap_err().var, "AGENTD_S3_CREATE_BUCKET");
}

#[test]
fn an_endpoint_is_tls_or_on_a_private_name() {
    for var in ["AGENTD_AWS_ENDPOINT", "AGENTD_SECRETS_ENDPOINT"] {
        for ok in ["https://s3.example.com", "http://localstack:4566", "http://127.0.0.1:4566"] {
            assert!(config(&[(var, ok)]).is_ok(), "{var}={ok}");
        }
        for bad in ["http://s3.example.com", "http://10.1.2.3:4566", "s3.example.com"] {
            let err = config(&[(var, bad)]).unwrap_err();
            assert_eq!(err.var, var, "{var}={bad}");
        }
    }
}

#[test]
fn telemetry_is_off_and_logs_are_text_unless_configured() {
    let c = config(&[]).unwrap();
    assert_eq!((c.telemetry.otlp, c.telemetry.json_logs), (false, false));
    let c =
        config(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4317"), ("AGENTD_LOG_FORMAT", "json")])
            .unwrap();
    assert_eq!((c.telemetry.otlp, c.telemetry.json_logs), (true, true));
    let err = config(&[("AGENTD_LOG_FORMAT", "xml")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_LOG_FORMAT");
}

#[test]
fn the_pool_is_sized_by_config_and_rejects_nonsense() {
    let c = config(&[("AGENTD_DB_POOL_SIZE", "32"), ("AGENTD_DB_POOL_WAIT_MS", "250")]).unwrap();
    let StoreConfig::Postgres { pool, .. } = c.store;
    assert_eq!((pool.max_size, pool.wait), (32, std::time::Duration::from_millis(250)));
    assert_eq!(config(&[("AGENTD_DB_POOL_SIZE", "0")]).unwrap_err().var, "AGENTD_DB_POOL_SIZE");
}

#[test]
fn the_secret_store_needs_a_kms_key() {
    for missing in ["", "  "] {
        let err = config(&[("AGENTD_SECRETS_KMS_KEY_ID", missing)]).unwrap_err();
        assert_eq!(err.var, "AGENTD_SECRETS_KMS_KEY_ID");
    }
}

#[test]
fn aws_secret_options_come_from_config() {
    let c = config(&[
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
        AwsSecretsConfig {
            prefix: "agentd/prod".into(),
            kms_key_id: "arn:aws:kms:eu-west-1:111122223333:key/k".into(),
            region: Some("eu-west-1".into()),
            endpoint: Some("https://secretsmanager.eu-west-1.amazonaws.com".into()),
            role_arn: Some("arn:aws:iam::111122223333:role/agentd-secrets".into()),
            recovery_window_days: 30,
        }
    );
    for days in ["-1", "0", "6", "31"] {
        let err = config(&[("AGENTD_SECRETS_RECOVERY_DAYS", days)]).unwrap_err();
        assert_eq!(err.var, "AGENTD_SECRETS_RECOVERY_DAYS", "{days}");
    }
}

mod streams;
