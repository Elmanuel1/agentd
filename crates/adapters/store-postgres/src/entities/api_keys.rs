//! `ApiKeysStore` on Postgres via Diesel. Keys are not tenant-scoped rows: a
//! request is resolved to its tenant by its key's digest.

use ::api_keys::{ApiKey, ApiKeysStore, StoreError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{OrgId, Timestamp, WorkspaceId};

use crate::schema::api_keys;
use crate::{diesel_be, pool_be, PgBackend};

/// Postgres-backed workspace API keys (Diesel).
#[derive(Debug, Clone)]
pub struct PgApiKeysStore {
    backend: PgBackend,
}

impl PgApiKeysStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = api_keys)]
struct NewApiKey<'a> {
    api_key_id: &'a str,
    org_id: &'a str,
    workspace_id: &'a str,
    name: &'a str,
    key_digest: &'a [u8],
    partial_key_hint: &'a str,
    created_at: DateTime<Utc>,
}

type Row = (String, String, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>);

const COLUMNS: (
    api_keys::api_key_id,
    api_keys::org_id,
    api_keys::workspace_id,
    api_keys::name,
    api_keys::partial_key_hint,
    api_keys::created_at,
    api_keys::revoked_at,
) = (
    api_keys::api_key_id,
    api_keys::org_id,
    api_keys::workspace_id,
    api_keys::name,
    api_keys::partial_key_hint,
    api_keys::created_at,
    api_keys::revoked_at,
);

fn record((id, org, workspace, name, hint, created_at, revoked_at): Row) -> ApiKey {
    ApiKey {
        id,
        org: OrgId::new(org),
        workspace: WorkspaceId::new(workspace),
        name,
        partial_key_hint: hint,
        created_at: Timestamp::from_datetime(created_at),
        revoked_at: revoked_at.map(Timestamp::from_datetime),
    }
}

#[async_trait]
impl ApiKeysStore for PgApiKeysStore {
    async fn insert(&self, key: &ApiKey, digest: &[u8]) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        diesel::insert_into(api_keys::table)
            .values(NewApiKey {
                api_key_id: &key.id,
                org_id: key.org.as_str(),
                workspace_id: key.workspace.as_str(),
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

    async fn find_active(&self, digest: &[u8]) -> Result<Option<ApiKey>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: Option<Row> = api_keys::table
            .filter(api_keys::key_digest.eq(digest))
            .filter(api_keys::revoked_at.is_null())
            .select(COLUMNS)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        Ok(row.map(record))
    }

    async fn list(&self) -> Result<Vec<ApiKey>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<Row> = api_keys::table
            .order((api_keys::created_at.asc(), api_keys::api_key_id.asc()))
            .select(COLUMNS)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(rows.into_iter().map(record).collect())
    }

    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let affected = diesel::update(
            api_keys::table.filter(api_keys::api_key_id.eq(id)).filter(api_keys::revoked_at.is_null()),
        )
        .set(api_keys::revoked_at.eq(at.as_datetime()))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(affected > 0)
    }

    async fn revoke_workspace(
        &self,
        org: &OrgId,
        workspace: &WorkspaceId,
        at: Timestamp,
    ) -> Result<usize, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        diesel::update(
            api_keys::table
                .filter(api_keys::org_id.eq(org.as_str()))
                .filter(api_keys::workspace_id.eq(workspace.as_str()))
                .filter(api_keys::revoked_at.is_null()),
        )
        .set(api_keys::revoked_at.eq(at.as_datetime()))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)
    }
}
