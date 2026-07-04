//! `SkillsStore` on Postgres — skill + version metadata rows; SKILL.md content
//! goes to object storage (never in the DB), keyed by `(skill_id, version)`.

use std::sync::Arc;

use agentd_blobs::BlobStore;
use agentd_core::WorkspaceId;
use agentd_skills::{Skill, SkillId, SkillVersion, SkillsStore, StoreError};
use async_trait::async_trait;
use bytes::Bytes;
use sqlx::types::Json;

use crate::{sqlx_msg, PgBackend};

/// Postgres + object-storage backed skills store.
#[derive(Clone)]
pub struct PgSkillsStore {
    backend: PgBackend,
    blobs: Arc<dyn BlobStore>,
}

impl std::fmt::Debug for PgSkillsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgSkillsStore").finish_non_exhaustive()
    }
}

impl PgSkillsStore {
    /// Wrap a pool + blob store.
    pub fn new(backend: PgBackend, blobs: Arc<dyn BlobStore>) -> Self {
        Self { backend, blobs }
    }

    fn key(ws: &WorkspaceId, id: &SkillId, version: u64) -> String {
        format!("skills/{}/{}/{version}", ws.0, id.as_str())
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

fn blob_be(e: agentd_blobs::BlobError) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[async_trait]
impl SkillsStore for PgSkillsStore {
    async fn insert(&self, ws: &WorkspaceId, s: Skill) -> Result<(), StoreError> {
        sqlx::query(
            "insert into skills (workspace_id, skill_id, display_name, created_at, doc)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(&ws.0)
        .bind(s.id.as_str())
        .bind(&s.display_name)
        .bind(s.created_at.as_datetime())
        .bind(Json(&s))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError> {
        let row: Option<(Json<Skill>,)> =
            sqlx::query_as("select doc from skills where workspace_id = $1 and skill_id = $2")
                .bind(&ws.0)
                .bind(id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::SkillNotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, s: Skill) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update skills set display_name = $3, doc = $4
             where workspace_id = $1 and skill_id = $2",
        )
        .bind(&ws.0)
        .bind(s.id.as_str())
        .bind(&s.display_name)
        .bind(Json(&s))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::SkillNotFound);
        }
        Ok(())
    }

    async fn delete(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError> {
        let versions = self.list_versions(ws, id).await?;
        let row: Option<(Json<Skill>,)> = sqlx::query_as(
            "delete from skills where workspace_id = $1 and skill_id = $2 returning doc",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        let skill = row.map(|(d,)| d.0).ok_or(StoreError::SkillNotFound)?;
        sqlx::query("delete from skill_versions where workspace_id = $1 and skill_id = $2")
            .bind(&ws.0)
            .bind(id.as_str())
            .execute(&self.backend.pool)
            .await
            .map_err(be)?;
        for v in versions {
            self.blobs
                .delete(&Self::key(ws, id, v.version))
                .await
                .map_err(blob_be)?;
        }
        Ok(skill)
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Skill>, StoreError> {
        let rows: Vec<(Json<Skill>,)> = sqlx::query_as(
            "select doc from skills where workspace_id = $1 order by created_at desc",
        )
        .bind(&ws.0)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }

    async fn push_version(&self, ws: &WorkspaceId, v: SkillVersion) -> Result<(), StoreError> {
        let exists: Option<(i32,)> =
            sqlx::query_as("select 1 from skills where workspace_id = $1 and skill_id = $2")
                .bind(&ws.0)
                .bind(v.skill_id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
        if exists.is_none() {
            return Err(StoreError::SkillNotFound);
        }
        self.blobs
            .put(
                &Self::key(ws, &v.skill_id, v.version),
                Bytes::from(v.content.clone().into_bytes()),
            )
            .await
            .map_err(blob_be)?;
        let mut meta = v.clone();
        meta.content = String::new();
        sqlx::query(
            "insert into skill_versions (workspace_id, skill_id, version, created_at, doc)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(&ws.0)
        .bind(v.skill_id.as_str())
        .bind(v.version as i64)
        .bind(v.created_at.as_datetime())
        .bind(Json(&meta))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: u64,
    ) -> Result<SkillVersion, StoreError> {
        let row: Option<(Json<SkillVersion>,)> = sqlx::query_as(
            "select doc from skill_versions
             where workspace_id = $1 and skill_id = $2 and version = $3",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .bind(version as i64)
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        let mut v = row.map(|(d,)| d.0).ok_or(StoreError::VersionNotFound)?;
        let bytes = self
            .blobs
            .get(&Self::key(ws, id, version))
            .await
            .map_err(blob_be)?;
        v.content = String::from_utf8_lossy(&bytes).into_owned();
        Ok(v)
    }

    async fn delete_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: u64,
    ) -> Result<SkillVersion, StoreError> {
        let existing = self.get_version(ws, id, version).await?;
        sqlx::query(
            "delete from skill_versions
             where workspace_id = $1 and skill_id = $2 and version = $3",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .bind(version as i64)
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        self.blobs
            .delete(&Self::key(ws, id, version))
            .await
            .map_err(blob_be)?;
        Ok(existing)
    }

    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
    ) -> Result<Vec<SkillVersion>, StoreError> {
        // Metadata only (content unset) — the service populates content on the
        // single-version full view via `get_version`.
        let rows: Vec<(Json<SkillVersion>,)> = sqlx::query_as(
            "select doc from skill_versions where workspace_id = $1 and skill_id = $2
             order by version asc",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }
}
