//! Stores and a scheduler that record or refuse what a delete asks of them.

use async_trait::async_trait;
use parking_lot::Mutex;
use shared::OrgId;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::secrets::{SecretError, SecretKey, SecretStore, SecretString};
use shared::ports::{BoxFuture, FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort};

use crate::registry::{NativeOrganizationsStore, Removed, StoreError};

/// Every key a fake was asked to delete.
type Recorded = Mutex<Vec<String>>;

pub(super) struct Rows(pub(super) Removed);

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
pub(super) struct Secrets {
    pub(super) deleted: Recorded,
    pub(super) refused: Option<String>,
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
pub(super) struct Blobs {
    pub(super) deleted: Recorded,
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

/// A scheduler that is down.
pub(super) struct Down;

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
