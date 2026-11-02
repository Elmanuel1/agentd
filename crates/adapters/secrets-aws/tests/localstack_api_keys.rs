//! `AwsApiKeys` against LocalStack Secrets Manager and KMS. Runs when
//! `AGENTD_AWS_ENDPOINT` names the emulator, with any static credentials.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use std::sync::Arc;

use common::{endpoint, id, manager};
use secrets_aws::{AwsApiKeys, AwsSecretStore};
use shared::ports::keys::{KeyPepper, KeySource};
use shared::ports::secrets::ExposeSecret;

/// A key store of its own, so a listing sees only this test's keys.
async fn keys(endpoint: &str) -> (AwsApiKeys, AwsSecretStore, String) {
    let prefix = format!("agentd_test/{}", id("k"));
    let store = common::store(endpoint, &prefix).await;
    let pepper = store.api_key_pepper().await.unwrap();
    (AwsApiKeys::new(store.clone(), Arc::new(pepper)), store, prefix)
}

#[tokio::test]
async fn a_created_key_resolves_until_it_is_revoked() {
    let Some(endpoint) = endpoint() else { return };
    let (keys, _, _) = keys(&endpoint).await;
    let ws = id("wrkspc");
    let (record, key) = keys.create("org_keys", &ws, "ci").await.unwrap();
    assert!(record.id.starts_with("apikey_01") && !record.revoked);
    assert!(key.expose_secret().starts_with("sk-ant-api03-"));
    let ctx = keys.resolve(key.expose_secret()).await.unwrap().unwrap();
    assert_eq!(
        (ctx.org.as_str(), ctx.workspace.as_str(), ctx.api_key_id.as_str()),
        ("org_keys", &*ws, &*record.id)
    );
    let listed = keys.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (&listed[0].id, &listed[0].name, &listed[0].partial_key_hint),
        (&record.id, &record.name, &record.partial_key_hint)
    );
    assert!(keys.revoke(&record.id).await.unwrap());
    assert_eq!(
        keys.resolve(key.expose_secret()).await.unwrap(),
        None,
        "a revoke holds from the next request"
    );
    assert!(!keys.revoke(&record.id).await.unwrap(), "revoking twice finds no active key");
    assert!(keys.list().await.unwrap()[0].revoked);
}

#[tokio::test]
async fn revoke_finds_only_the_exact_id() {
    let Some(endpoint) = endpoint() else { return };
    let (keys, _, _) = keys(&endpoint).await;
    let (record, key) = keys.create("org_keys", &id("wrkspc"), "ci").await.unwrap();
    // Secrets Manager matches tag values by prefix; a prefix of the id is not the id.
    assert!(!keys.revoke(&record.id[..record.id.len() - 1]).await.unwrap());
    assert!(keys.resolve(key.expose_secret()).await.unwrap().is_some());
}

#[tokio::test]
async fn only_the_exact_key_under_the_same_pepper_resolves() {
    let Some(endpoint) = endpoint() else { return };
    let (keys, store, _) = keys(&endpoint).await;
    let (_, key) = keys.create("org_keys", &id("wrkspc"), "ci").await.unwrap();
    let key = key.expose_secret();
    let other = AwsApiKeys::new(store, Arc::new(KeyPepper::new(&[9; 32])));
    assert_eq!(other.resolve(key).await.unwrap(), None, "another pepper verifies nothing");
    let mut altered = key.to_owned();
    altered.replace_range(20..21, if &key[20..21] == "A" { "B" } else { "A" });
    for wrong in [altered.as_str(), "", "sk-ant-api03-x", &key[..key.len() - 1]] {
        assert_eq!(keys.resolve(wrong).await.unwrap(), None, "{wrong}");
    }
}

#[tokio::test]
async fn a_key_secret_holds_only_its_digest_and_tenant() {
    let Some(endpoint) = endpoint() else { return };
    let (keys, store, prefix) = keys(&endpoint).await;
    let (record, key) = keys.create("org_keys", &id("wrkspc"), "ci").await.unwrap();
    let client = manager(&endpoint).await;
    let listed = client.list_secrets().send().await.unwrap().secret_list.unwrap_or_default();
    let entry = listed
        .iter()
        .find(|e| e.name.as_deref().is_some_and(|n| n.starts_with(&format!("{prefix}/system/api_keys/"))))
        .unwrap();
    assert_eq!(entry.kms_key_id.as_deref(), Some(store.key_arn()), "under the configured key");
    let value = client.get_secret_value().secret_id(entry.arn.clone().unwrap()).send().await.unwrap();
    let value = value.secret_string.unwrap();
    let key = key.expose_secret();
    assert!(!value.contains(&key[13..]), "the key is not stored");
    let json: serde_json::Value = serde_json::from_str(&value).unwrap();
    assert_eq!(json["api_key_id"], record.id.as_str());
    assert_eq!(json["digest"].as_str().unwrap().len(), 64);
    assert!(!entry.name.as_deref().unwrap().contains(&key[13..]));
}

#[tokio::test]
async fn bad_ids_and_names_are_refused() {
    let Some(endpoint) = endpoint() else { return };
    let (keys, _, _) = keys(&endpoint).await;
    for (org, ws, name) in [
        ("org/x", "wrkspc_a", "n"),
        ("org_a", "wrkspc a", "n"),
        ("org_a", "wrkspc_a", ""),
        ("org_a", "wrkspc_a", "a\nb"),
    ] {
        assert!(keys.create(org, ws, name).await.is_err(), "{org} {ws} {name:?}");
    }
    assert!(keys.list().await.unwrap().is_empty());
}
