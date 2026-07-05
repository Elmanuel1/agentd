//! `SkillsStore` on Postgres via Diesel — skill + version metadata rows;
//! SKILL.md content goes to object storage, keyed by `(skill_id, version)`.

use std::sync::Arc;

use agentd_blobs::BlobStore;
use agentd_core::WorkspaceId;
use agentd_skills::{Skill, SkillId, SkillVersion, SkillsStore, StoreError};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;

use crate::schema::{skill_versions, skills};
use crate::{diesel_be, pool_be, PgBackend};

/// Postgres + object-storage backed skills store (Diesel).
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
    /// Wrap a backend + blob store.
    pub fn new(backend: PgBackend, blobs: Arc<dyn BlobStore>) -> Self {
        Self { backend, blobs }
    }

    fn key(ws: &WorkspaceId, id: &SkillId, version: u64) -> String {
        format!("skills/{}/{}/{version}", ws.0, id.as_str())
    }
}

fn blob_be(e: agentd_blobs::BlobError) -> StoreError {
    StoreError::Backend(e.to_string())
}

fn parse<T: serde::de::DeserializeOwned>(doc: serde_json::Value) -> Result<T, StoreError> {
    serde_json::from_value(doc).map_err(|e| StoreError::Backend(e.to_string()))
}

fn to_doc<T: serde::Serialize>(v: &T) -> Result<serde_json::Value, StoreError> {
    serde_json::to_value(v).map_err(|e| StoreError::Backend(e.to_string()))
}

#[derive(diesel::Insertable)]
#[diesel(table_name = skills)]
struct NewSkill {
    workspace_id: String,
    skill_id: String,
    display_name: String,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = skill_versions)]
struct NewSkillVersion {
    workspace_id: String,
    skill_id: String,
    version: i64,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[async_trait]
impl SkillsStore for PgSkillsStore {
    async fn insert(&self, ws: &WorkspaceId, s: Skill) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(skills::table)
            .values(NewSkill {
                workspace_id: ws.0.clone(),
                skill_id: s.id.as_str().to_owned(),
                display_name: s.display_name.clone(),
                created_at: s.created_at.as_datetime(),
                doc: to_doc(&s)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = skills::table
            .filter(skills::workspace_id.eq(&ws.0))
            .filter(skills::skill_id.eq(id.as_str()))
            .select(skills::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse(doc.ok_or(StoreError::SkillNotFound)?)
    }

    async fn replace(&self, ws: &WorkspaceId, s: Skill) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::update(
            skills::table
                .filter(skills::workspace_id.eq(&ws.0))
                .filter(skills::skill_id.eq(s.id.as_str())),
        )
        .set((skills::display_name.eq(&s.display_name), skills::doc.eq(to_doc(&s)?)))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::SkillNotFound);
        }
        Ok(())
    }

    async fn delete(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError> {
        let versions = self.list_versions(ws, id).await?;
        let skill = self.get(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::delete(
            skills::table
                .filter(skills::workspace_id.eq(&ws.0))
                .filter(skills::skill_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        diesel::delete(
            skill_versions::table
                .filter(skill_versions::workspace_id.eq(&ws.0))
                .filter(skill_versions::skill_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        for v in versions {
            self.blobs
                .delete(&Self::key(ws, id, v.version))
                .await
                .map_err(blob_be)?;
        }
        Ok(skill)
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Skill>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = skills::table
            .filter(skills::workspace_id.eq(&ws.0))
            .order(skills::created_at.desc())
            .select(skills::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse).collect()
    }

    async fn push_version(&self, ws: &WorkspaceId, v: SkillVersion) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = skills::table
            .filter(skills::workspace_id.eq(&ws.0))
            .filter(skills::skill_id.eq(v.skill_id.as_str()))
            .select(skills::skill_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
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
        diesel::insert_into(skill_versions::table)
            .values(NewSkillVersion {
                workspace_id: ws.0.clone(),
                skill_id: v.skill_id.as_str().to_owned(),
                version: v.version as i64,
                created_at: v.created_at.as_datetime(),
                doc: to_doc(&meta)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get_version(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
        version: u64,
    ) -> Result<SkillVersion, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = skill_versions::table
            .filter(skill_versions::workspace_id.eq(&ws.0))
            .filter(skill_versions::skill_id.eq(id.as_str()))
            .filter(skill_versions::version.eq(version as i64))
            .select(skill_versions::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        let mut v: SkillVersion = parse(doc.ok_or(StoreError::VersionNotFound)?)?;
        let bytes = self.blobs.get(&Self::key(ws, id, version)).await.map_err(blob_be)?;
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
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::delete(
            skill_versions::table
                .filter(skill_versions::workspace_id.eq(&ws.0))
                .filter(skill_versions::skill_id.eq(id.as_str()))
                .filter(skill_versions::version.eq(version as i64)),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        self.blobs.delete(&Self::key(ws, id, version)).await.map_err(blob_be)?;
        Ok(existing)
    }

    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        id: &SkillId,
    ) -> Result<Vec<SkillVersion>, StoreError> {
        // Metadata only (content unset) — content fetched on the full view.
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = skill_versions::table
            .filter(skill_versions::workspace_id.eq(&ws.0))
            .filter(skill_versions::skill_id.eq(id.as_str()))
            .order(skill_versions::version.asc())
            .select(skill_versions::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse).collect()
    }
}
