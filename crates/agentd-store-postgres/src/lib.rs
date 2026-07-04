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

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub use entities::agents::PgAgentsStore;
pub use entities::deployments::PgDeploymentsStore;
pub use entities::files::PgFilesStore;

/// Shared connection pool.
#[derive(Clone)]
pub struct PgBackend {
    /// The sqlx pool.
    pub pool: PgPool,
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
        Ok(Self { pool })
    }
}

/// Map any sqlx error to a store `Backend` string (each entity wraps it in
/// its own `StoreError::Backend`).
pub(crate) fn sqlx_msg(e: sqlx::Error) -> String {
    e.to_string()
}
