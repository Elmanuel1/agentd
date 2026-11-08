//! `SkillVersionsStore` on Postgres via Diesel: a skill's version rows and
//! their bundles in object storage.

use std::sync::Arc;

use ::skills::{SkillId, SkillVersion, SkillVersionsStore, StoreError};
use async_trait::async_trait;
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::ports::blobs::BlobStore;
use shared::{Pagination, WorkspaceId};

use super::skills::{version_col, PgSkillsStore};
use crate::schema::skill_versions;
use crate::{blob_be, diesel_be, parse_doc, pool_be, PgBackend};

/// Postgres-backed skill versions store (Diesel).
#[derive(Clone)]
pub struct PgSkillVersionsStore {
    backend: PgBackend,
    blobs: Arc<dyn BlobStore>,
}

impl std::fmt::Debug for PgSkillVersionsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgSkillVersionsStore").finish_non_exhaustive()
    }
}

impl PgSkillVersionsStore {
    /// Wrap a backend + blob store.
    pub fn new(backend: PgBackend, blobs: Arc<dyn BlobStore>) -> Self {
        Self { backend, blobs }
    }
}

#[async_trait]
impl SkillVersionsStore for PgSkillVersionsStore {
    async fn versions(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        page: &Pagination,
    ) -> Result<Vec<SkillVersion>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = skill_versions::table
            .filter(skill_versions::workspace_id.eq(ws.as_str()))
            .filter(skill_versions::skill_id.eq(id.as_str()))
            .order(skill_versions::version.desc())
            .select(skill_versions::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn delete_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: &str,
    ) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::delete(
            skill_versions::table
                .filter(skill_versions::workspace_id.eq(ws.as_str()))
                .filter(skill_versions::skill_id.eq(id.as_str()))
                .filter(skill_versions::version.eq(version_col(version))),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::VersionNotFound);
        }
        self.blobs.delete(&PgSkillsStore::key(ws, id, version)).await.map_err(blob_be)?;
        let remaining: i64 = skill_versions::table
            .filter(skill_versions::workspace_id.eq(ws.as_str()))
            .filter(skill_versions::skill_id.eq(id.as_str()))
            .count()
            .get_result(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(remaining > 0)
    }
}
