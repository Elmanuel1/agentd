//! The cleanup sweep: secrets and blobs live outside Postgres, so a removal that fails after a
//! delete commits is retried here. Once an hour, on the one replica holding its lock, every secret
//! no `credentials` row names and every blob no `files`, `skill_versions` or `memory_versions` row
//! names is removed, unless it was written in the last day: a secret or blob is written before its
//! row commits. Each store is walked a page at a time.

use std::sync::Arc;
use std::time::Duration;

use diesel::QueryableByName;
use diesel::sql_types::{Array, BigInt, Text};
use diesel_async::RunQueryDsl;
use sessions::retention::RunLock;
use sessions::store::StoreError;
use shared::Timestamp;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::secrets::{SecretKey, SecretStore};

use crate::entities::blob_keys::{self, Kind};
use crate::entities::run_lock::PgRunLock;
use crate::{PgBackend, blob_be, diesel_be, pool_be, secret_be};

const EVERY: Duration = Duration::from_secs(3600);
const GRACE: Duration = Duration::from_secs(24 * 3600);
const LOCK: &str = "cleanup_sweep";
const KINDS: [Kind; 3] = [Kind::File, Kind::SkillVersion, Kind::MemoryVersion];

#[derive(QueryableByName)]
struct Unnamed {
    #[diesel(sql_type = BigInt)]
    i: i64,
}

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

    /// Try a run every hour.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut every = tokio::time::interval(EVERY);
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
            let old: Vec<SecretKey> =
                page.items.into_iter().filter(|s| s.written_at < before).map(|s| s.key).collect();
            let parts = old.iter().map(|k| [k.workspace(), k.vault(), k.credential()].map(str::to_owned));
            for key in self.unnamed(Join::Credentials, parts.collect()).await?.filter_map(|i| old.get(i)) {
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
                for (key, _, _) in self.unnamed(Join::Blob(kind), parts).await?.filter_map(|i| of_kind.get(i))
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

    /// The positions in `parts` of the keys no row names.
    async fn unnamed(
        &self,
        join: Join,
        parts: Vec<[String; 3]>,
    ) -> Result<impl Iterator<Item = usize>, StoreError> {
        let (mut a, mut b, mut c) = (vec![], vec![], vec![]);
        for [pa, pb, pc] in parts {
            a.push(pa);
            b.push(pb);
            c.push(pc);
        }
        let sql = format!(
            "SELECT k.i - 1 AS i FROM unnest($1::text[], $2::text[], $3::text[]) WITH ORDINALITY AS k(a, b, c, i) {}",
            join.sql()
        );
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<Unnamed> = match a.is_empty() {
            true => vec![],
            false => diesel::sql_query(sql)
                .bind::<Array<Text>, _>(a)
                .bind::<Array<Text>, _>(b)
                .bind::<Array<Text>, _>(c)
                .load(&mut conn)
                .await
                .map_err(diesel_be)?,
        };
        Ok(rows.into_iter().filter_map(|r| usize::try_from(r.i).ok()))
    }
}

/// The rows that name a key's parts `k.a`, `k.b` and `k.c`.
#[derive(Clone, Copy)]
enum Join {
    Credentials,
    Blob(Kind),
}

impl Join {
    fn sql(self) -> &'static str {
        match self {
            Self::Credentials => {
                "LEFT JOIN credentials r ON r.workspace_id = k.a AND r.vault_id = k.b AND r.credential_id = k.c
                 WHERE r.credential_id IS NULL"
            }
            Self::Blob(Kind::File) => {
                "LEFT JOIN files r ON r.workspace_id = k.a AND r.file_id = k.b WHERE r.file_id IS NULL"
            }
            Self::Blob(Kind::SkillVersion) => {
                "LEFT JOIN skill_versions r ON r.workspace_id = k.a AND r.skill_id = k.b AND r.doc->>'id' = k.c
                 WHERE r.skill_id IS NULL"
            }
            Self::Blob(Kind::MemoryVersion) => {
                "LEFT JOIN memory_versions r ON r.workspace_id = k.a AND r.store_id = k.b AND r.version_id = k.c
                 WHERE r.version_id IS NULL"
            }
        }
    }
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
