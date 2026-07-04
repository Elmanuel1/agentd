//! agentd-store-postgres — PostgreSQL persistence behind the entity store
//! traits.
//!
//! Chosen over DynamoDB when the product is **dashboard-first**: list /
//! filter / sort / search / count are plain SQL (see the `list` queries that
//! use `DISTINCT ON` and indexed columns), instead of a second search copy.
//! Still stateless across replicas — all state is in Postgres — and the
//! multi-replica **exactly-once** claim is a native
//! `INSERT … ON CONFLICT DO NOTHING`.
//!
//! Schema is managed by **Flyway** (`migrations/V*.sql`, Postgres 16 /
//! Flyway 11). This crate only *uses* the migrated schema — it never creates
//! tables — so schema and code evolve through reviewed migrations, matching
//! the tosspaper convention.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod entities;
pub mod schema;

use diesel_async::pooled_connection::deadpool::Pool as DieselPool;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::AsyncPgConnection;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

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

/// Shared connection pools. Transitional: `pool` (sqlx) serves stores not yet
/// ported; `diesel` (diesel-async) serves the ported ones. Both target the
/// same database. When every store is on Diesel, `pool` is removed.
#[derive(Clone)]
pub struct PgBackend {
    /// The sqlx pool (legacy stores).
    pub pool: PgPool,
    /// The diesel-async pool (ported stores).
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
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
            .map_err(|e| PgError::Connect(e.to_string()))?;
        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(database_url);
        let diesel = DieselPool::builder(manager)
            .max_size(10)
            .build()
            .map_err(|e| PgError::Connect(e.to_string()))?;
        Ok(Self { pool, diesel })
    }
}

/// Map any sqlx error to a store `Backend` string (each entity wraps it in
/// its own `StoreError::Backend`).
pub(crate) fn sqlx_msg(e: sqlx::Error) -> String {
    e.to_string()
}

/// Map a Diesel query error to a store `Backend` variant. Generic over the
/// entity `StoreError` so every entity can reuse it.
pub(crate) fn diesel_be<E: FromBackend>(e: diesel::result::Error) -> E {
    E::backend(e.to_string())
}

/// Map a diesel-async pool checkout error to a store `Backend` variant.
pub(crate) fn pool_be<E: FromBackend>(
    e: diesel_async::pooled_connection::deadpool::PoolError,
) -> E {
    E::backend(e.to_string())
}

/// Entity `StoreError`s that carry a `Backend(String)` variant.
pub(crate) trait FromBackend {
    fn backend(msg: String) -> Self;
}

impl FromBackend for agentd_agents::store::StoreError {
    fn backend(msg: String) -> Self {
        Self::Backend(msg)
    }
}

/// Serialize a snake_case enum to its wire string for a filter column
/// (e.g. `EnvironmentState::Active` → `"active"`). Falls back to `""` for
/// non-string serializations, which never happens for these tag enums.
pub(crate) fn enum_str<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
