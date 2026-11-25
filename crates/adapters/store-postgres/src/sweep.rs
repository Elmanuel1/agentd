//! The cleanup sweep: secrets and blobs live outside Postgres, so a removal that fails after a
//! delete commits is retried here. Once an hour, on the one replica holding its lock, every secret
//! no `credentials` row names and every blob no `files`, `skill_versions` or `memory_versions` row
//! names is removed, unless it was written in the last day: a secret or blob is written before its
//! row commits. Each store is walked a page at a time, and only keys in the form agentd writes are
//! read as its own: the bucket and the secret prefix belong to one agentd database.

use std::sync::Arc;
use std::time::Duration;

use sessions::retention::RunLock;
use sessions::store::StoreError;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::secrets::{SecretKey, SecretStore};
use shared::{Id, Timestamp};
use vaults::types::{CredentialResource, VaultResource};

use crate::entities::blob_keys::{self, Kind};
use crate::entities::run_lock::PgRunLock;
use crate::{PgBackend, blob_be, secret_be};

mod unnamed;
use unnamed::{Join, unnamed};

const EVERY: Duration = Duration::from_secs(3600);
const GRACE: Duration = Duration::from_secs(24 * 3600);
const LOCK: &str = "cleanup_sweep";
const KINDS: [Kind; 3] = [Kind::File, Kind::SkillVersion, Kind::MemoryVersion];

/// Removes what no row names from the secret and blob stores.
pub struct Sweep {
    backend: PgBackend,
    secrets: Arc<dyn SecretStore>,
    blobs: Arc<dyn BlobStore>,
    lock: PgRunLock,
}

impl std::fmt::Debug for Sweep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sweep").finish_non_exhaustive()
    }
}

impl Sweep {
    /// Over `backend`'s rows and the stores outside it.
    pub fn new(backend: PgBackend, secrets: Arc<dyn SecretStore>, blobs: Arc<dyn BlobStore>) -> Self {
        let lock = PgRunLock::new(backend.clone());
        Self { backend, secrets, blobs, lock }
    }

    /// Sweep once if no other replica is sweeping, leaving what was written after `now` less a day;
    /// `false` while another replica holds the lock.
    pub async fn run(&self, now: Timestamp) -> Result<bool, StoreError> {
        let before = now.earlier_by(GRACE);
        let pass = async move {
            let secrets = self.secrets(before).await;
            let blobs = self.blobs(before).await;
            let (secrets, blobs) = (secrets?, blobs?);
            tracing::info!(
                removed_secrets = secrets.removed,
                removed_blobs = blobs.removed,
                failed = secrets.failed + blobs.failed,
                "sweep: removed the secrets and blobs no row names"
            );
            Ok(())
        };
        self.lock.exclusive(LOCK, Box::pin(pass)).await
    }

    /// Try a run every hour, the first an hour after the start.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut every = tokio::time::interval_at(tokio::time::Instant::now() + EVERY, EVERY);
            loop {
                every.tick().await;
                if let Err(e) = self.run(Timestamp::now()).await {
                    tracing::warn!(error = %e, "sweep: the run failed; the next one tries again");
                }
            }
        })
    }

    async fn secrets(&self, before: Timestamp) -> Result<Removed, StoreError> {
        let (mut removed, mut after) = (Removed::default(), None);
        loop {
            let page = self.secrets.list(after).await.map_err(secret_be)?;
            let old: Vec<SecretKey> = page
                .items
                .into_iter()
                .filter(|s| s.written_at < before && agentd_wrote(&s.key))
                .map(|s| s.key)
                .collect();
            let parts = old.iter().map(|k| [k.workspace(), k.vault(), k.credential()].map(str::to_owned));
            for key in
                unnamed(&self.backend, Join::Credentials, parts.collect()).await?.filter_map(|i| old.get(i))
            {
                let deleted = self.secrets.delete(key).await.map_err(|e| e.to_string());
                removed.count(&key.to_string(), deleted);
            }
            after = page.next;
            if after.is_none() {
                return Ok(removed);
            }
        }
    }

    async fn blobs(&self, before: Timestamp) -> Result<Removed, StoreError> {
        let (mut removed, mut after) = (Removed::default(), None);
        loop {
            let page = self.blobs.list("", after).await.map_err(blob_be)?;
            let old: Vec<(String, Kind, [String; 3])> = page
                .items
                .into_iter()
                .filter(|b| b.written_at < before)
                .filter_map(|b| blob_keys::parse(&b.key).map(|(kind, parts)| (b.key, kind, parts)))
                .collect();
            for kind in KINDS {
                let of_kind: Vec<&(String, Kind, [String; 3])> = old.iter().filter(|b| b.1 == kind).collect();
                let parts = of_kind.iter().map(|b| b.2.clone()).collect();
                for (key, _, _) in
                    unnamed(&self.backend, Join::Blob(kind), parts).await?.filter_map(|i| of_kind.get(i))
                {
                    let deleted = match self.blobs.delete(key).await {
                        Ok(()) | Err(BlobError::NotFound(_)) => Ok(()),
                        Err(e) => Err(e.to_string()),
                    };
                    removed.count(key, deleted);
                }
            }
            after = page.next;
            if after.is_none() {
                return Ok(removed);
            }
        }
    }
}

/// Whether `key` names a vault and credential in the form agentd makes them.
fn agentd_wrote(key: &SecretKey) -> bool {
    Id::<VaultResource>::parse(key.vault()).is_ok()
        && Id::<CredentialResource>::parse(key.credential()).is_ok()
}

/// What one pass removed, and how many removals failed.
#[derive(Default)]
struct Removed {
    removed: usize,
    failed: usize,
}

impl Removed {
    /// Count the removal of `key`; a failure is logged and the pass goes on.
    fn count(&mut self, key: &str, deleted: Result<(), String>) {
        match deleted {
            Ok(()) => self.removed += 1,
            Err(e) => {
                self.failed += 1;
                tracing::warn!(key, error = %e, "sweep: could not remove; the next run tries again");
            }
        }
    }
}
