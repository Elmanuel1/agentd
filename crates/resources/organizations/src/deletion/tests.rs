use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use shared::OrgId;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::secrets::{SecretError, SecretKey, SecretStore, SecretString};
use shared::ports::{
    BoxFuture, FakeScheduler, FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort,
    TenantRef,
};

use super::{Deleted, OrganizationDeletion};
use crate::registry::{NativeOrganizationsStore, Removed, StoreError};

/// Every key a fake was asked to delete.
type Recorded = Mutex<Vec<String>>;

struct Rows(Removed);

impl NativeOrganizationsStore for Rows {
    async fn create(&self, _: &OrgId) -> Result<(), StoreError> {
        Ok(())
    }
    async fn delete(&self, org: &OrgId) -> Result<Removed, StoreError> {
        match org.as_str() {
            "org_gone" => Err(StoreError::NotFound("org_gone".into())),
            _ => Ok(self.0.clone()),
        }
    }
}

/// Deletes every secret but `refused`.
#[derive(Default)]
struct Secrets {
    deleted: Recorded,
    refused: Option<String>,
}

#[async_trait]
impl SecretStore for Secrets {
    async fn put(&self, _: &SecretKey, _: &SecretString) -> Result<(), SecretError> {
        Ok(())
    }
    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        Err(SecretError::NotFound(key.to_string()))
    }
    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        if self.refused.as_deref() == Some(key.to_string().as_str()) {
            return Err(SecretError::Backend("refused".into()));
        }
        self.deleted.lock().push(key.to_string());
        Ok(())
    }
}

/// Deletes every blob but `gone`, already gone, and `refused`, which fails.
#[derive(Default)]
struct Blobs {
    deleted: Recorded,
}

impl BlobStore for Blobs {
    fn put<'a>(&'a self, _: &'a str, _: bytes::Bytes) -> BoxFuture<'a, Result<(), BlobError>> {
        Box::pin(async { Ok(()) })
    }
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<bytes::Bytes, BlobError>> {
        Box::pin(async move { Err(BlobError::NotFound(key.into())) })
    }
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<(), BlobError>> {
        Box::pin(async move {
            match key {
                "gone" => Err(BlobError::NotFound(key.into())),
                "refused" => Err(BlobError::Backend("refused".into())),
                _ => {
                    self.deleted.lock().push(key.to_owned());
                    Ok(())
                }
            }
        })
    }
}

fn removed(blobs: &[&str]) -> Removed {
    Removed {
        workspaces: vec!["wrkspc_a".into()],
        rows: vec![("credentials", 2)],
        secrets: vec![
            SecretKey::new("wrkspc_a", "vlt_a", "vcrd_a").unwrap(),
            SecretKey::new("wrkspc_a", "vlt_a", "vcrd_b").unwrap(),
        ],
        blobs: blobs.iter().map(|b| (*b).to_owned()).collect(),
        dreams: vec![FireTarget::new("dream", "drm_a"), FireTarget::new("dream", "drm_b")],
    }
}

async fn scheduled(target: &FireTarget) -> Arc<FakeScheduler> {
    let scheduler = Arc::new(FakeScheduler::new());
    let spec = ScheduleSpec {
        target: target.clone(),
        cron: "0 * * * *".into(),
        timezone: "UTC".into(),
        tenant: TenantRef {
            org_id: "org_a".into(),
            workspace_id: "wrkspc_a".into(),
            api_key_id: "apikey_a".into(),
        },
    };
    scheduler.upsert(spec).await.unwrap();
    scheduler
}

#[tokio::test]
async fn a_delete_removes_the_secrets_blobs_and_dream_schedules_its_rows_named() {
    let (secrets, blobs) = (Arc::new(Secrets::default()), Arc::new(Blobs::default()));
    let scheduler = scheduled(&FireTarget::new("dream", "drm_a")).await;
    let removed = removed(&["wrkspc_a/file_a", "gone"]);
    let deletion = OrganizationDeletion::new(
        Arc::new(Rows(removed.clone())),
        secrets.clone(),
        blobs.clone(),
        scheduler.clone(),
    );
    let deleted = deletion.delete(&OrgId::new("org_a")).await.unwrap();
    let gone =
        ["secret wrkspc_a/vlt_a/vcrd_a", "secret wrkspc_a/vlt_a/vcrd_b", "blob wrkspc_a/file_a", "blob gone"]
            .into_iter()
            .chain(["schedule dream/drm_a", "schedule dream/drm_b"])
            .map(str::to_owned)
            .collect();
    assert_eq!(deleted, Deleted { removed, gone, left: vec![] });
    assert_eq!(*secrets.deleted.lock(), ["wrkspc_a/vlt_a/vcrd_a", "wrkspc_a/vlt_a/vcrd_b"]);
    assert_eq!(*blobs.deleted.lock(), ["wrkspc_a/file_a"]);
    assert!(scheduler.registered().is_empty(), "the dream's schedule is gone; one already gone counts");
}

#[tokio::test]
async fn what_could_not_be_removed_is_named_and_the_rest_still_goes() {
    let secrets = Arc::new(Secrets { refused: Some("wrkspc_a/vlt_a/vcrd_a".into()), ..Secrets::default() });
    let blobs = Arc::new(Blobs::default());
    let deletion = OrganizationDeletion::new(
        Arc::new(Rows(removed(&["refused", "wrkspc_a/file_b"]))),
        secrets.clone(),
        blobs.clone(),
        Arc::new(FakeScheduler::new()),
    );
    let deleted = deletion.delete(&OrgId::new("org_a")).await.unwrap();
    assert_eq!(
        deleted.left,
        ["secret wrkspc_a/vlt_a/vcrd_a: secret backend: refused", "blob refused: blob backend: refused"]
    );
    assert_eq!(*secrets.deleted.lock(), ["wrkspc_a/vlt_a/vcrd_b"]);
    assert_eq!(*blobs.deleted.lock(), ["wrkspc_a/file_b"]);
}

/// A scheduler that is down.
struct Down;

#[async_trait]
impl SchedulerPort for Down {
    async fn upsert(&self, _: ScheduleSpec) -> Result<(), SchedulerError> {
        Err(SchedulerError::Unavailable("down".into()))
    }
    async fn pause(&self, _: &FireTarget) -> Result<(), SchedulerError> {
        Err(SchedulerError::Unavailable("down".into()))
    }
    async fn unpause(&self, _: &FireTarget) -> Result<(), SchedulerError> {
        Err(SchedulerError::Unavailable("down".into()))
    }
    async fn delete(&self, _: &FireTarget) -> Result<(), SchedulerError> {
        Err(SchedulerError::Unavailable("down".into()))
    }
    async fn describe(&self, _: &FireTarget) -> Result<ScheduleState, SchedulerError> {
        Err(SchedulerError::Unavailable("down".into()))
    }
}

#[tokio::test]
async fn a_dream_schedule_the_scheduler_cannot_delete_is_named() {
    let deletion = OrganizationDeletion::new(
        Arc::new(Rows(removed(&[]))),
        Arc::new(Secrets::default()),
        Arc::new(Blobs::default()),
        Arc::new(Down),
    );
    let deleted = deletion.delete(&OrgId::new("org_a")).await.unwrap();
    assert_eq!(
        deleted.left,
        [
            "schedule dream/drm_a: scheduler unavailable: down",
            "schedule dream/drm_b: scheduler unavailable: down"
        ]
    );
}

#[tokio::test]
async fn an_unknown_organization_removes_nothing() {
    let secrets = Arc::new(Secrets::default());
    let deletion = OrganizationDeletion::new(
        Arc::new(Rows(removed(&["wrkspc_a/file_a"]))),
        secrets.clone(),
        Arc::new(Blobs::default()),
        Arc::new(FakeScheduler::new()),
    );
    let err = deletion.delete(&OrgId::new("org_gone")).await.unwrap_err();
    assert!(matches!(err, StoreError::NotFound(id) if id == "org_gone"));
    assert!(secrets.deleted.lock().is_empty());
}

#[test]
fn a_new_id_is_the_one_given_or_a_generated_org_id() {
    assert_eq!(crate::registry::new_id(Some("org_sdk")).as_str(), "org_sdk");
    let generated = crate::registry::new_id(None);
    assert!(generated.as_str().starts_with("org_01"), "{}", generated.as_str());
}
