//! The cleanup sweep: secrets and blobs live outside Postgres, so a removal that fails after a
//! delete commits is retried here. Once an hour, on the one replica holding its lock, every secret
//! no `credentials` row names and every blob no `files`, `skill_versions` or `memory_versions` row
//! names is removed, unless it was written in the last day: a secret or blob is written before its
//! row commits.

use std::sync::Arc;
use std::time::Duration;

use diesel::QueryableByName;
use diesel::sql_types::{Array, Text};
use diesel_async::RunQueryDsl;
use sessions::retention::RunLock;
use sessions::store::StoreError;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::secrets::{SecretKey, SecretStore};
use shared::{BackendError, Timestamp};

use crate::entities::run_lock::PgRunLock;
use crate::{PgBackend, diesel_be, pool_be};

const EVERY: Duration = Duration::from_secs(3600);
const GRACE: Duration = Duration::from_secs(24 * 3600);
const LOCK: &str = "cleanup_sweep";
/// Keys checked against their table in one statement.
const BATCH: usize = 1000;

/// For each blob kind, its key prefix and the rows that name its keys' parts.
const BLOBS: [(&str, &str); 3] = [
    (
        "skills/",
        "LEFT JOIN skill_versions r ON r.workspace_id = k.a AND r.skill_id = k.b AND r.doc->>'id' = k.c
                 WHERE r.skill_id IS NULL",
    ),
    (
        "memory/",
        "LEFT JOIN memory_versions r ON r.workspace_id = k.a AND r.store_id = k.b AND r.version_id = k.c
                 WHERE r.version_id IS NULL",
    ),
    ("", "LEFT JOIN files r ON r.workspace_id = k.a AND r.file_id = k.b WHERE r.file_id IS NULL"),
];
const CREDENTIALS: &str =
    "LEFT JOIN credentials r ON r.workspace_id = k.a AND r.vault_id = k.b AND r.credential_id = k.c
     WHERE r.credential_id IS NULL";

#[derive(QueryableByName)]
struct Unnamed {
    #[diesel(sql_type = Text)]
    key: String,
}

/// A listed key and its parts, as the query that finds the rows naming it reads them.
struct Listed {
    key: String,
    parts: [String; 3],
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
        let listed = self.secrets.list().await.map_err(backend)?;
        let old: Vec<Listed> = listed
            .into_iter()
            .filter(|s| s.written_at < before)
            .map(|s| Listed {
                parts: [s.key.workspace(), s.key.vault(), s.key.credential()].map(str::to_owned),
                key: s.key.to_string(),
            })
            .collect();
        let mut removed = Removed::default();
        for key in self.unnamed(CREDENTIALS, old).await? {
            let deleted = match key.parse::<SecretKey>() {
                Ok(secret) => self.secrets.delete(&secret).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            removed.count(&key, deleted);
        }
        Ok(removed)
    }

    async fn blobs(&self, before: Timestamp) -> Result<Removed, StoreError> {
        let listed = self.blobs.list("").await.map_err(backend)?;
        let old: Vec<String> = listed.into_iter().filter(|b| b.written_at < before).map(|b| b.key).collect();
        let mut removed = Removed::default();
        for (prefix, join) in BLOBS {
            let of_kind: Vec<Listed> = old.iter().filter_map(|key| kind(key, prefix)).collect();
            for key in self.unnamed(join, of_kind).await? {
                let deleted = match self.blobs.delete(&key).await {
                    Ok(()) | Err(BlobError::NotFound(_)) => Ok(()),
                    Err(e) => Err(e.to_string()),
                };
                removed.count(&key, deleted);
            }
        }
        Ok(removed)
    }

    /// The keys of `listed` no row names, by `join` over `unnest(...) AS k(key, a, b, c)`, a batch at a
    /// time.
    async fn unnamed(&self, join: &str, listed: Vec<Listed>) -> Result<Vec<String>, StoreError> {
        let sql = format!(
            "SELECT k.key FROM unnest($1::text[], $2::text[], $3::text[], $4::text[]) AS k(key, a, b, c) {join}"
        );
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let mut unnamed = vec![];
        let mut rest = listed.into_iter().peekable();
        while rest.peek().is_some() {
            let (mut keys, mut a, mut b, mut c) = (vec![], vec![], vec![], vec![]);
            for Listed { key, parts: [pa, pb, pc] } in rest.by_ref().take(BATCH) {
                keys.push(key);
                a.push(pa);
                b.push(pb);
                c.push(pc);
            }
            let rows: Vec<Unnamed> = diesel::sql_query(&sql)
                .bind::<Array<Text>, _>(keys)
                .bind::<Array<Text>, _>(a)
                .bind::<Array<Text>, _>(b)
                .bind::<Array<Text>, _>(c)
                .load(&mut conn)
                .await
                .map_err(diesel_be)?;
            unnamed.extend(rows.into_iter().map(|r| r.key));
        }
        Ok(unnamed)
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

/// A blob key and its parts under `prefix`: two for a file (`<workspace>/<file>`), three for the
/// others; none for a key of another kind.
fn kind(key: &str, prefix: &str) -> Option<Listed> {
    let rest = key.strip_prefix(prefix)?;
    let parts: Vec<&str> = rest.split('/').collect();
    let parts = match (prefix, parts.as_slice()) {
        ("", [ws, file]) => [(*ws).to_owned(), (*file).to_owned(), String::new()],
        ("", _) => return None,
        (_, [ws, owner, version]) => [(*ws).to_owned(), (*owner).to_owned(), (*version).to_owned()],
        _ => return None,
    };
    Some(Listed { key: key.to_owned(), parts })
}

fn backend(e: impl std::error::Error + Send + Sync + 'static) -> StoreError {
    StoreError::Backend(BackendError::new(e))
}

#[cfg(test)]
mod tests {
    use super::kind;

    #[test]
    fn each_blob_key_reads_as_its_kind_only() {
        let parts = |a: &str, b: &str, c: &str| Some([a.to_owned(), b.to_owned(), c.to_owned()]);
        assert_eq!(
            kind("skills/wrkspc_a/skill_a/skver_a", "skills/"),
            parts("wrkspc_a", "skill_a", "skver_a")
        );
        assert_eq!(
            kind("memory/wrkspc_a/memstore_a/memver_a", "memory/"),
            parts("wrkspc_a", "memstore_a", "memver_a")
        );
        assert_eq!(kind("wrkspc_a/file_a", ""), parts("wrkspc_a", "file_a", ""));
        assert_eq!(kind("skills/wrkspc_a/skill_a/skver_a", ""), None);
        assert_eq!(kind("memory/wrkspc_a/x", "memory/"), None);
    }
}
