//! `OrganizationsStore` on Postgres: the `organizations` rows.

mod delete;

use ::organizations::registry::{OrganizationsStore, Removed, StoreError};
use async_trait::async_trait;
use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;
use shared::OrgId;

use crate::{PgBackend, diesel_be, pool_be};

/// Postgres-backed organizations store.
#[derive(Debug, Clone)]
pub struct PgOrganizationsStore {
    backend: PgBackend,
}

impl PgOrganizationsStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl OrganizationsStore for PgOrganizationsStore {
    async fn create(&self, org: &OrgId) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let inserted =
            diesel::sql_query("INSERT INTO organizations (org_id) VALUES ($1) ON CONFLICT DO NOTHING")
                .bind::<Text, _>(org.as_str())
                .execute(&mut *conn)
                .await
                .map_err(diesel_be)?;
        match inserted {
            0 => Err(StoreError::Exists(org.as_str().to_owned())),
            _ => Ok(()),
        }
    }

    async fn delete(&self, org: &OrgId) -> Result<Removed, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        delete::organization(&mut conn, org).await
    }
}
