use parking_lot::Mutex;

use shared::ports::keys::KeyPepper;

use super::*;
use crate::store::KeyDigests;

/// Keys held in memory, with their digests.
#[derive(Default)]
struct MemoryKeys(Mutex<Vec<(EnvironmentKey, Vec<u8>)>>);

#[async_trait]
impl KeyDigests<EnvironmentKey> for MemoryKeys {
    async fn find_active(
        &self,
        digests: &[Vec<u8>],
    ) -> Result<Option<(EnvironmentKey, Vec<u8>)>, crate::StoreError> {
        let keys = self.0.lock();
        Ok(keys.iter().find(|(k, d)| digests.contains(d) && k.revoked_at.is_none()).cloned())
    }

    async fn rehash(&self, id: &str, digest: &[u8]) -> Result<(), crate::StoreError> {
        let mut keys = self.0.lock();
        keys.iter_mut().filter(|(k, _)| k.id == id).for_each(|(_, d)| *d = digest.to_vec());
        Ok(())
    }
}

#[async_trait]
impl EnvironmentKeysStore for MemoryKeys {
    async fn insert(&self, key: &EnvironmentKey, digest: &[u8]) -> Result<(), crate::StoreError> {
        self.0.lock().push((key.clone(), digest.to_vec()));
        Ok(())
    }

    async fn list(&self) -> Result<Vec<EnvironmentKey>, crate::StoreError> {
        Ok(self.0.lock().iter().map(|(k, _)| k.clone()).collect())
    }

    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, crate::StoreError> {
        let mut keys = self.0.lock();
        let found = keys.iter_mut().find(|(k, _)| k.id == id && k.revoked_at.is_none());
        Ok(found.map(|(k, _)| k.revoked_at = Some(at)).is_some())
    }
}

fn service() -> EnvironmentKeysService {
    EnvironmentKeysService::new(
        Arc::new(MemoryKeys::default()),
        Arc::new(PepperRing::single(KeyPepper::new(&[7; 32]))),
    )
}

#[tokio::test]
async fn a_key_resolves_to_its_tenant_and_environment_until_it_is_revoked() {
    let keys = service();
    let (record, key) = keys.create("org_a", "wrkspc_a", "env_a", "worker").await.unwrap();
    assert!(key.expose_secret().starts_with("sk-ant-oat01-") && record.id.starts_with("envkey_"));
    assert!(record.partial_key_hint.ends_with(&key.expose_secret()[key.expose_secret().len() - 4..]));
    let Some(EnvironmentGrant::Environment(ctx, caller)) = keys.resolve(key.expose_secret()).await.unwrap()
    else {
        panic!("an environment key grants its environment")
    };
    assert_eq!((ctx.org.as_str(), ctx.workspace.as_str()), ("org_a", "wrkspc_a"));
    assert_eq!((ctx.api_key_id, caller.queue), (record.id.clone(), "env_a".to_owned()));
    assert!(keys.revoke(&record.id).await.unwrap());
    assert!(keys.resolve(key.expose_secret()).await.unwrap().is_none());
    assert!(!keys.revoke(&record.id).await.unwrap());
}

#[tokio::test]
async fn a_workspace_key_or_a_malformed_id_is_refused() {
    let keys = service();
    let workspace = material::generate(KeyKind::Workspace);
    assert!(keys.resolve(workspace.expose_secret()).await.unwrap().is_none());
    assert!(keys.create("org a", "wrkspc_a", "env_a", "w").await.is_err());
    assert!(keys.create("org_a", "wrkspc_a", "env/a", "w").await.is_err());
    assert!(keys.create("org_a", "wrkspc_a", "env_a", "").await.is_err());
    for reserved in ["runtime"] {
        assert!(keys.create("org_a", "wrkspc_a", reserved, "w").await.is_err(), "{reserved}");
    }
}

#[tokio::test]
async fn each_runtime_key_grants_the_runtime_queue_and_no_other_key_does() {
    let (runtime, previous) =
        (material::generate(KeyKind::Environment), material::generate(KeyKind::Environment));
    let keys = service().with_runtime_keys(&[runtime.clone(), previous.clone()]);
    assert_eq!(keys.resolve(runtime.expose_secret()).await.unwrap(), Some(EnvironmentGrant::Runtime));
    assert_eq!(keys.resolve(previous.expose_secret()).await.unwrap(), Some(EnvironmentGrant::Runtime));
    let other = material::generate(KeyKind::Environment);
    assert_eq!(keys.resolve(other.expose_secret()).await.unwrap(), None);
    assert_eq!(service().resolve(runtime.expose_secret()).await.unwrap(), None, "no runtime key held");
}
