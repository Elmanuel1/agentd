//! `SkillsStore` on Postgres via Diesel: skill rows, created with their first
//! version; the uploaded zip bundle goes to object storage, keyed by
//! `(skill_id, version)`.

use std::sync::Arc;

use ::skills::{Skill, SkillId, SkillVersion, SkillsStore, StoreError};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::ports::blobs::BlobStore;
use shared::{Pagination, WorkspaceId};

use crate::schema::{skill_versions, skills};
use crate::{blob_be, diesel_be, parse_doc, pool_be, to_doc, PgBackend};

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

    /// A bundle's object key, by the version's id: two versions numbered in
    /// the same microsecond still get their own objects.
    pub(crate) fn key(ws: &WorkspaceId, id: &SkillId, version_id: &str) -> String {
        format!("skills/{}/{}/{version_id}", ws.as_str(), id.as_str())
    }
}

// The `version` column is bigint; the wire `version` is that integer as a
// string. `doc` keeps the string form for a byte-exact echo.
pub(crate) fn version_col(version: &str) -> i64 {
    version.parse().unwrap_or(0)
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
    async fn create(
        &self,
        ws: &WorkspaceId,
        skill: Skill,
        version: SkillVersion,
        bundle: Bytes,
    ) -> Result<(), StoreError> {
        self.blobs.put(&Self::key(ws, &skill.id, version.id.as_str()), bundle).await.map_err(blob_be)?;
        let new_skill = NewSkill {
            workspace_id: ws.as_str().to_owned(),
            skill_id: skill.id.as_str().to_owned(),
            display_name: skill.display_name.clone(),
            created_at: skill.created_at.as_datetime(),
            doc: to_doc(&skill)?,
        };
        let new_version = NewSkillVersion {
            workspace_id: ws.as_str().to_owned(),
            skill_id: version.skill_id.as_str().to_owned(),
            version: version_col(&version.version),
            created_at: version.created_at.as_datetime(),
            doc: to_doc(&version)?,
        };
        // A skill and its first version land together or not at all.
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                diesel::insert_into(skills::table).values(new_skill).execute(c).await?;
                diesel::insert_into(skill_versions::table).values(new_version).execute(c).await?;
                Ok(())
            }
            .scope_boxed()
        })
        .await
        .map_err(diesel_be)
    }

    async fn get(&self, ws: &WorkspaceId, id: &SkillId) -> Result<Skill, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = skills::table
            .filter(skills::workspace_id.eq(ws.as_str()))
            .filter(skills::skill_id.eq(id.as_str()))
            .select(skills::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::SkillNotFound)?)
    }

    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Skill>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = skills::table
            .filter(skills::workspace_id.eq(ws.as_str()))
            .order(skills::created_at.desc())
            .select(skills::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn replace(&self, ws: &WorkspaceId, skill: Skill) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let affected = diesel::update(
            skills::table
                .filter(skills::workspace_id.eq(ws.as_str()))
                .filter(skills::skill_id.eq(skill.id.as_str())),
        )
        .set(skills::doc.eq(to_doc(&skill)?))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::SkillNotFound);
        }
        Ok(())
    }

    async fn delete_skill(&self, ws: &WorkspaceId, id: &SkillId) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        // Every version's blob key must be removed — fetch the full set, not a page.
        let version_docs: Vec<serde_json::Value> = skill_versions::table
            .filter(skill_versions::workspace_id.eq(ws.as_str()))
            .filter(skill_versions::skill_id.eq(id.as_str()))
            .select(skill_versions::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        let versions: Vec<SkillVersion> =
            version_docs.into_iter().map(parse_doc).collect::<Result<_, StoreError>>()?;
        diesel::delete(
            skill_versions::table
                .filter(skill_versions::workspace_id.eq(ws.as_str()))
                .filter(skill_versions::skill_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        let affected = diesel::delete(
            skills::table
                .filter(skills::workspace_id.eq(ws.as_str()))
                .filter(skills::skill_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::SkillNotFound);
        }
        for v in versions {
            self.blobs.delete(&Self::key(ws, id, v.id.as_str())).await.map_err(blob_be)?;
        }
        Ok(())
    }
}
