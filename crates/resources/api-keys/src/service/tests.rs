use parking_lot::Mutex;

use shared::ports::keys::KeyPepper;

use super::*;

/// Keys held in memory, with their digests.
#[derive(Default)]
struct MemoryKeys(Mutex<Vec<(ApiKey, Vec<u8>)>>);

#[async_trait]
impl KeyDigests<ApiKey> for MemoryKeys {
    async fn find_active(&self, digests: &[Vec<u8>]) -> Result<Option<(ApiKey, Vec<u8>)>, StoreError> {
        let keys = self.0.lock();
        Ok(keys.iter().find(|(k, d)| digests.contains(d) && k.revoked_at.is_none()).cloned())
    }

    async fn rehash(&self, id: &str, digest: &[u8]) -> Result<(), StoreError> {
        let mut keys = self.0.lock();
        keys.iter_mut().filter(|(k, _)| k.id == id).for_each(|(_, d)| *d = digest.to_vec());
        Ok(())
    }
}

#[async_trait]
impl ApiKeysStore for MemoryKeys {
    async fn insert(&self, key: &ApiKey, digest: &[u8]) -> Result<(), StoreError> {
        self.0.lock().push((key.clone(), digest.to_vec()));
        Ok(())
    }

    async fn list(&self) -> Result<Vec<ApiKey>, StoreError> {
        Ok(self.0.lock().iter().map(|(k, _)| k.clone()).collect())
    }

    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, StoreError> {
        let mut keys = self.0.lock();
        let found = keys.iter_mut().find(|(k, _)| k.id == id && k.revoked_at.is_none());
        Ok(found.map(|(k, _)| k.revoked_at = Some(at)).is_some())
    }

    async fn revoke_workspace(
        &self,
        org: &OrgId,
        workspace: &WorkspaceId,
        at: Timestamp,
    ) -> Result<usize, StoreError> {
        let mut keys = self.0.lock();
        let mine = keys
            .iter_mut()
            .filter(|(k, _)| &k.org == org && &k.workspace == workspace && k.revoked_at.is_none());
        Ok(mine.map(|(k, _)| k.revoked_at = Some(at)).count())
    }
}

fn ring(pepper: u8) -> Arc<PepperRing> {
    Arc::new(PepperRing::single(KeyPepper::new(&[pepper; 32])))
}

fn service(pepper: u8) -> (ApiKeysService, Arc<MemoryKeys>) {
    let store = Arc::new(MemoryKeys::default());
    (ApiKeysService::new(store.clone(), ring(pepper)), store)
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
    let other = ApiKeysService::new(store, ring(2));
    assert_eq!(other.resolve(key.expose_secret()).await.unwrap(), None);
}

#[tokio::test]
async fn a_key_under_an_older_pepper_resolves_and_moves_to_the_current_one() {
    let old = shared::hash::KeyRing::parse(&format!("p1:{}", "11".repeat(32))).unwrap();
    let both =
        shared::hash::KeyRing::parse(&format!("p2:{},p1:{}", "22".repeat(32), "11".repeat(32))).unwrap();
    let store = Arc::new(MemoryKeys::default());
    let before = ApiKeysService::new(store.clone(), Arc::new(PepperRing::from_ring(&old)));
    let (_, key) = before.create("org_a", "wrkspc_a", "ci").await.unwrap();
    let under_old = store.0.lock()[0].1.clone();

    let rotated = PepperRing::from_ring(&both);
    let current = rotated.digests(key.expose_secret())[0].clone();
    let after = ApiKeysService::new(store.clone(), Arc::new(rotated));
    assert!(after.resolve(key.expose_secret()).await.unwrap().is_some());
    let stored = store.0.lock()[0].1.clone();
    assert_eq!(stored, current);
    assert_ne!(stored, under_old);
    // The old pepper retired, the key still resolves.
    let new_only = shared::hash::KeyRing::parse(&format!("p2:{}", "22".repeat(32))).unwrap();
    let retired = ApiKeysService::new(store, Arc::new(PepperRing::from_ring(&new_only)));
    assert!(retired.resolve(key.expose_secret()).await.unwrap().is_some());
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
