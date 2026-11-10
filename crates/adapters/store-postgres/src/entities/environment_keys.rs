//! `EnvironmentKeysStore` on Postgres via Diesel. A request is resolved to
//! its tenant and environment by its key's digest.

use ::api_keys::{EnvironmentKey, EnvironmentKeysStore, StoreError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{OrgId, Timestamp, WorkspaceId};

use crate::schema::environment_keys;
use crate::{diesel_be, pool_be, PgBackend};

/// Postgres-backed environment keys (Diesel).
#[derive(Debug, Clone)]
pub struct PgEnvironmentKeysStore {
    backend: PgBackend,
}

impl PgEnvironmentKeysStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = environment_keys)]
struct NewEnvironmentKey<'a> {
    environment_key_id: &'a str,
    org_id: &'a str,
    workspace_id: &'a str,
    environment_id: &'a str,
    name: &'a str,
    key_digest: &'a [u8],
    partial_key_hint: &'a str,
    created_at: DateTime<Utc>,
}

type Row = (String, String, String, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>);

const COLUMNS: (
    environment_keys::environment_key_id,
    environment_keys::org_id,
    environment_keys::workspace_id,
    environment_keys::environment_id,
    environment_keys::name,
    environment_keys::partial_key_hint,
    environment_keys::created_at,
    environment_keys::revoked_at,
) = (
    environment_keys::environment_key_id,
    environment_keys::org_id,
    environment_keys::workspace_id,
    environment_keys::environment_id,
    environment_keys::name,
    environment_keys::partial_key_hint,
    environment_keys::created_at,
    environment_keys::revoked_at,
);

fn record((id, org, workspace, environment_id, name, hint, created_at, revoked_at): Row) -> EnvironmentKey {
    EnvironmentKey {
        id,
        org: OrgId::new(org),
        workspace: WorkspaceId::new(workspace),
        environment_id,
        name,
        partial_key_hint: hint,
        created_at: Timestamp::from_datetime(created_at),
        revoked_at: revoked_at.map(Timestamp::from_datetime),
    }
}

#[async_trait]
impl EnvironmentKeysStore for PgEnvironmentKeysStore {
    async fn insert(&self, key: &EnvironmentKey, digest: &[u8]) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        diesel::insert_into(environment_keys::table)
            .values(NewEnvironmentKey {
                environment_key_id: &key.id,
                org_id: key.org.as_str(),
                workspace_id: key.workspace.as_str(),
                environment_id: &key.environment_id,
                name: &key.name,
                key_digest: digest,
                partial_key_hint: &key.partial_key_hint,
                created_at: key.created_at.as_datetime(),
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn find_active(&self, digest: &[u8]) -> Result<Option<EnvironmentKey>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: Option<Row> = environment_keys::table
            .filter(environment_keys::key_digest.eq(digest))
            .filter(environment_keys::revoked_at.is_null())
            .select(COLUMNS)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        Ok(row.map(record))
    }

    async fn list(&self) -> Result<Vec<EnvironmentKey>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<Row> = environment_keys::table
            .order((environment_keys::created_at.asc(), environment_keys::environment_key_id.asc()))
            .select(COLUMNS)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(rows.into_iter().map(record).collect())
    }

    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let affected = diesel::update(
            environment_keys::table
                .filter(environment_keys::environment_key_id.eq(id))
                .filter(environment_keys::revoked_at.is_null()),
        )
        .set(environment_keys::revoked_at.eq(at.as_datetime()))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(affected > 0)
    }
}
