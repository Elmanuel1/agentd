//! store-postgres — PostgreSQL persistence behind the entity store
//! traits.
//!
//! Chosen over DynamoDB when the product is **dashboard-first**: list /
//! filter / sort / search / count are plain SQL (see the `list` queries that
//! use `DISTINCT ON` and indexed columns), instead of a second search copy.
//! Still stateless across replicas — all state is in Postgres — and the
//! multi-replica **exactly-once** claim is a native
//! `INSERT … ON CONFLICT DO NOTHING`.
//!
//! Queries use **Diesel** (typed DSL over `crate::schema`, the jOOQ analog):
//! codegen-style typed schema + compile-time-checked queries, no ORM runtime
//! behaviour. **Flyway** owns the migrations (`migrations/V*.sql`, Postgres 16
//! / Flyway 11) and `crate::schema` mirrors them — the same Flyway → jOOQ
//! split as the tosspaper Java stack. This crate never creates tables.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod entities;
pub mod schema;

use diesel_async::pooled_connection::deadpool::Pool as DieselPool;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::AsyncPgConnection;

/// The diesel-async connection pool type (tokio-postgres under the hood).
pub type DieselPgPool = DieselPool<AsyncPgConnection>;

pub use entities::agents::PgAgentsStore;
pub use entities::deployments::PgDeploymentsStore;
pub use entities::dreams::PgDreamsStore;
pub use entities::environments::PgEnvironmentsStore;
pub use entities::files::PgFilesStore;
pub use entities::memory::PgMemoryStore;
pub use entities::sessions::PgSessionsStore;
pub use entities::skills::PgSkillsStore;
pub use entities::vaults::PgVaultsStore;

/// Shared diesel-async connection pool (tokio-postgres). All entity stores go
/// through Diesel — the typed schema lives in `crate::schema`.
#[derive(Clone)]
pub struct PgBackend {
    /// The diesel-async pool.
    pub diesel: DieselPgPool,
}

impl std::fmt::Debug for PgBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgBackend").finish_non_exhaustive()
    }
}

/// Connection errors.
#[derive(Debug, thiserror::Error)]
pub enum PgError {
    /// Could not connect.
    #[error("connect: {0}")]
    Connect(String),
}

impl PgBackend {
    /// Connect using `database_url` (or `DATABASE_URL`), pooled.
    pub async fn connect(database_url: &str) -> Result<Self, PgError> {
        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(database_url);
        let diesel =
            DieselPool::builder(manager).max_size(10).build().map_err(|e| PgError::Connect(e.to_string()))?;
        Ok(Self { diesel })
    }
}

/// Map a Diesel query error to a store `Backend` variant, preserving it as the
/// error's `source()`. Generic over the entity `StoreError` so every entity can
/// reuse it.
pub(crate) fn diesel_be<E: FromBackend>(e: diesel::result::Error) -> E {
    E::backend(shared::BackendError::new(e))
}

/// Map a diesel-async pool checkout error to a store `Backend` variant.
pub(crate) fn pool_be<E: FromBackend>(e: diesel_async::pooled_connection::deadpool::PoolError) -> E {
    E::backend(shared::BackendError::new(e))
}

/// Entity `StoreError`s that carry a `Backend(BackendError)` variant.
pub(crate) trait FromBackend {
    fn backend(source: shared::BackendError) -> Self;
}

macro_rules! from_backend {
    ($($t:ty),+ $(,)?) => {$(
        impl FromBackend for $t {
            fn backend(source: shared::BackendError) -> Self {
                Self::Backend(source)
            }
        }
    )+};
}

from_backend!(
    agents::store::StoreError,
    deployments::store::StoreError,
    files::store::StoreError,
    environments::store::StoreError,
    vaults::store::StoreError,
    skills::StoreError,
    sessions::store::StoreError,
    dreams::store::StoreError,
    memory::store::StoreError,
);

/// Serialize a snake_case enum to its wire string for a filter column
/// (e.g. `EnvironmentState::Active` → `"active"`). Falls back to `""` for
/// non-string serializations, which never happens for these tag enums.
pub(crate) fn enum_str<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

/// Deserialize a `doc` jsonb value into a wire type. Generic over the entity
/// `StoreError` (inferred from the call's `?` context).
pub(crate) fn parse_doc<T: serde::de::DeserializeOwned, E: FromBackend>(
    doc: serde_json::Value,
) -> Result<T, E> {
    serde_json::from_value(doc).map_err(|e| E::backend(shared::BackendError::new(e)))
}

/// Serialize a wire type into a `doc` jsonb value.
pub(crate) fn to_doc<T: serde::Serialize, E: FromBackend>(v: &T) -> Result<serde_json::Value, E> {
    serde_json::to_value(v).map_err(|e| E::backend(shared::BackendError::new(e)))
}

/// Map an object-store error into the entity `Backend` variant.
pub(crate) fn blob_be<E: FromBackend>(e: blobs_s3::BlobError) -> E {
    E::backend(shared::BackendError::new(e))
}
