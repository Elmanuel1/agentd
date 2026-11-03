use parking_lot::Mutex;

use super::*;

/// Keys held in memory, with their digests.
#[derive(Default)]
struct MemoryKeys(Mutex<Vec<(ApiKey, Vec<u8>)>>);

#[async_trait]
impl ApiKeysStore for MemoryKeys {
    async fn insert(&self, key: &ApiKey, digest: &[u8]) -> Result<(), StoreError> {
        self.0.lock().push((key.clone(), digest.to_vec()));
        Ok(())
    }

    async fn find_active(&self, digest: &[u8]) -> Result<Option<ApiKey>, StoreError> {
        let keys = self.0.lock();
        Ok(keys.iter().find(|(k, d)| d == digest && k.revoked_at.is_none()).map(|(k, _)| k.clone()))
    }

    async fn list(&self) -> Result<Vec<ApiKey>, StoreError> {
        Ok(self.0.lock().iter().map(|(k, _)| k.clone()).collect())
    }

    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, StoreError> {
        let mut keys = self.0.lock();
        let found = keys.iter_mut().find(|(k, _)| k.id == id && k.revoked_at.is_none());
        Ok(found.map(|(k, _)| k.revoked_at = Some(at)).is_some())
    }
}

fn service(pepper: u8) -> (ApiKeysService, Arc<MemoryKeys>) {
    let store = Arc::new(MemoryKeys::default());
    (ApiKeysService::new(store.clone(), Arc::new(KeyPepper::new(&[pepper; 32]))), store)
}

#[tokio::test]
async fn a_created_key_resolves_to_its_tenant_until_it_is_revoked() {
    let (keys, _) = service(1);
    let (record, key) = keys.create("org_a", "wrkspc_a", "ci").await.unwrap();
    assert!(record.id.starts_with("apikey_01"));
    assert!(key.expose_secret().starts_with("sk-ant-api03-"));
    let ctx = keys.resolve(key.expose_secret()).await.unwrap().unwrap();
    assert_eq!(
        (ctx.org.as_str(), ctx.workspace.as_str(), ctx.api_key_id.as_str()),
        ("org_a", "wrkspc_a", record.id.as_str())
    );
    assert!(keys.revoke(&record.id).await.unwrap());
    assert_eq!(keys.resolve(key.expose_secret()).await.unwrap(), None);
    assert!(!keys.revoke(&record.id).await.unwrap(), "revoking twice finds no active key");
    assert!(keys.list().await.unwrap()[0].revoked_at.is_some());
}

#[tokio::test]
async fn only_the_digest_is_stored_and_another_pepper_verifies_nothing() {
    let (keys, store) = service(1);
    let (_, key) = keys.create("org_a", "wrkspc_a", "ci").await.unwrap();
    let stored = store.0.lock()[0].1.clone();
    assert_eq!(stored.len(), 32);
    assert!(!String::from_utf8_lossy(&stored).contains(&key.expose_secret()[13..]));
    let other = ApiKeysService::new(store, Arc::new(KeyPepper::new(&[2; 32])));
    assert_eq!(other.resolve(key.expose_secret()).await.unwrap(), None);
}

#[tokio::test]
async fn an_admin_key_or_a_malformed_key_resolves_to_no_one() {
    let (keys, _) = service(1);
    let admin = material::generate(KeyKind::Admin);
    for key in [admin.expose_secret(), "", "sk-ant-api03-short"] {
        assert_eq!(keys.resolve(key).await.unwrap(), None, "{key}");
    }
}

#[tokio::test]
async fn bad_ids_and_names_are_refused() {
    let (keys, _) = service(1);
    for (org, ws, name) in [
        ("org/x", "wrkspc_a", "n"),
        ("org_a", "wrkspc a", "n"),
        ("org_a", "wrkspc_a", ""),
        ("org_a", "wrkspc_a", "a\nb"),
    ] {
        assert!(keys.create(org, ws, name).await.is_err(), "{org} {ws} {name:?}");
    }
    assert!(keys.create("org_a", "wrkspc_a", &"n".repeat(129)).await.is_err());
    assert!(keys.list().await.unwrap().is_empty());
}
