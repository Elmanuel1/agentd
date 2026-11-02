//! Config parsing tests.

use std::collections::HashMap;

use super::*;

fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
    let vars: HashMap<String, String> = vars.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
    Config::from_lookup(|k| vars.get(k).cloned())
}

#[test]
fn defaults_select_postgres_s3_blob_env_local_none() {
    let c = config(&[("AGENTD_API_KEYS", "k:org_a:wrkspc_a")]).unwrap();
    assert!(matches!(c.store, StoreConfig::Postgres { .. }));
    assert_eq!(c.blobs, BlobsConfig::S3(S3Config::new("agentd-artifacts")));
    assert_eq!(
        (c.secrets, c.scheduler, c.runtime),
        (SecretsConfig::Blob, SchedulerConfig::Local, RuntimeConfig::None)
    );
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
        ConfigError { var: "AGENTD_API_KEYS", reason: "entry 1 is not key:org:workspace".into() }
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
