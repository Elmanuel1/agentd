//! `AdminKeysStore` on Postgres via Diesel. A request to `/v1/organizations`
//! is resolved to its organization by its key's digest.

use ::api_keys::{AdminKey, AdminKeysStore, KeyDigests, StoreError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{OrgId, Timestamp};

use crate::schema::admin_keys;
use crate::{diesel_be, pool_be, PgBackend};

/// Postgres-backed organization admin keys (Diesel).
#[derive(Debug, Clone)]
pub struct PgAdminKeysStore {
    backend: PgBackend,
}

impl PgAdminKeysStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = admin_keys)]
struct NewAdminKey<'a> {
    admin_key_id: &'a str,
    org_id: &'a str,
    name: &'a str,
    key_digest: &'a [u8],
    partial_key_hint: &'a str,
    created_at: DateTime<Utc>,
}

type Row = (String, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>);

const COLUMNS: (
    admin_keys::admin_key_id,
    admin_keys::org_id,
    admin_keys::name,
    admin_keys::partial_key_hint,
    admin_keys::created_at,
    admin_keys::revoked_at,
) = (
    admin_keys::admin_key_id,
    admin_keys::org_id,
    admin_keys::name,
    admin_keys::partial_key_hint,
    admin_keys::created_at,
    admin_keys::revoked_at,
);

fn record((id, org, name, hint, created_at, revoked_at): Row) -> AdminKey {
    AdminKey {
        id,
        org: OrgId::new(org),
        name,
        partial_key_hint: hint,
        created_at: Timestamp::from_datetime(created_at),
        revoked_at: revoked_at.map(Timestamp::from_datetime),
    }
}

#[async_trait]
impl KeyDigests<AdminKey> for PgAdminKeysStore {
    async fn find_active(&self, digests: &[Vec<u8>]) -> Result<Option<(AdminKey, Vec<u8>)>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: Option<(Row, Vec<u8>)> = admin_keys::table
            .filter(admin_keys::key_digest.eq_any(digests))
            .filter(admin_keys::revoked_at.is_null())
            .select((COLUMNS, admin_keys::key_digest))
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        Ok(row.map(|(row, digest)| (record(row), digest)))
    }

    async fn rehash(&self, id: &str, digest: &[u8]) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        diesel::update(admin_keys::table.filter(admin_keys::admin_key_id.eq(id)))
            .set(admin_keys::key_digest.eq(digest))
            .execute(&mut conn)
            .await
            .map(drop)
            .map_err(diesel_be)
    }
}

#[async_trait]
impl AdminKeysStore for PgAdminKeysStore {
    async fn insert(&self, key: &AdminKey, digest: &[u8]) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        diesel::insert_into(admin_keys::table)
            .values(NewAdminKey {
                admin_key_id: &key.id,
                org_id: key.org.as_str(),
                name: &key.name,
                key_digest: digest,
                partial_key_hint: &key.partial_key_hint,
                created_at: key.created_at.as_datetime(),
            })
            .execute(&mut conn)
            .await
            .map(drop)
            .map_err(diesel_be)
    }

    async fn list(&self) -> Result<Vec<AdminKey>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<Row> = admin_keys::table
            .order((admin_keys::created_at.asc(), admin_keys::admin_key_id.asc()))
            .select(COLUMNS)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(rows.into_iter().map(record).collect())
    }

    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let affected = diesel::update(
            admin_keys::table
                .filter(admin_keys::admin_key_id.eq(id))
                .filter(admin_keys::revoked_at.is_null()),
        )
        .set(admin_keys::revoked_at.eq(at.as_datetime()))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(affected > 0)
    }
}
