//! `SkillVersionsStore` on Postgres via Diesel: a skill's version rows (the
//! version id lives in `doc`) and their bundles in object storage.

use std::sync::Arc;

use ::skills::{SkillId, SkillVersion, SkillVersionsStore, StoreError, VersionRef};
use async_trait::async_trait;
use bytes::Bytes;
use diesel::OptionalExtension;
use diesel::PgAnyJsonExpressionMethods;
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::ports::blobs::BlobStore;
use shared::{Pagination, WorkspaceId};

use super::skills::{version_col, PgSkillsStore};
use crate::schema::skill_versions;
use crate::{blob_be, diesel_be, parse_doc, pool_be, to_doc, PgBackend};

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

    async fn add(&self, ws: &WorkspaceId, version: SkillVersion, bundle: Bytes) -> Result<(), StoreError> {
        let key = PgSkillsStore::key(ws, &version.skill_id, &version.version);
        self.blobs.put(&key, bundle).await.map_err(blob_be)?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(skill_versions::table)
            .values((
                skill_versions::workspace_id.eq(ws.as_str()),
                skill_versions::skill_id.eq(version.skill_id.as_str()),
                skill_versions::version.eq(version_col(&version.version)),
                skill_versions::created_at.eq(version.created_at.as_datetime()),
                skill_versions::doc.eq(to_doc(&version)?),
            ))
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        which: &VersionRef,
    ) -> Result<SkillVersion, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let mut q = skill_versions::table
            .filter(skill_versions::workspace_id.eq(ws.as_str()))
            .filter(skill_versions::skill_id.eq(id.as_str()))
            .into_boxed();
        q = match which {
            VersionRef::Latest => q,
            VersionRef::Number(n) => q.filter(skill_versions::version.eq(version_col(n))),
            VersionRef::Id(vid) => {
                q.filter(skill_versions::doc.retrieve_as_text("id").eq(vid.as_str().to_owned()))
            }
        };
        let doc: Option<serde_json::Value> = q
            .order(skill_versions::version.desc())
            .select(skill_versions::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::VersionNotFound)?)
    }

    async fn count(&self, ws: &WorkspaceId, id: &SkillId) -> Result<usize, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let n: i64 = skill_versions::table
            .filter(skill_versions::workspace_id.eq(ws.as_str()))
            .filter(skill_versions::skill_id.eq(id.as_str()))
            .count()
            .get_result(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(usize::try_from(n).unwrap_or_default())
    }

    async fn bundle(&self, ws: &WorkspaceId, version: &SkillVersion) -> Result<Bytes, StoreError> {
        self.blobs.get(&PgSkillsStore::key(ws, &version.skill_id, &version.version)).await.map_err(blob_be)
    }

    async fn delete_version(&self, ws: &WorkspaceId, version: &SkillVersion) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::delete(
            skill_versions::table
                .filter(skill_versions::workspace_id.eq(ws.as_str()))
                .filter(skill_versions::skill_id.eq(version.skill_id.as_str()))
                .filter(skill_versions::version.eq(version_col(&version.version))),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::VersionNotFound);
        }
        self.blobs.delete(&PgSkillsStore::key(ws, &version.skill_id, &version.version)).await.map_err(blob_be)
    }
}
