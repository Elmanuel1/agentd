//! `DreamsStore` on Postgres via Diesel — dream rows + append-only runs.

use agentd_core::WorkspaceId;
use agentd_dreams::store::{DreamsStore, StoreError};
use agentd_dreams::types::{Dream, DreamId, DreamRun};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;

use crate::schema::{dream_runs, dreams};
use crate::{diesel_be, enum_str, parse_doc, pool_be, to_doc, PgBackend};

/// Postgres-backed dreams store (Diesel).
#[derive(Debug, Clone)]
pub struct PgDreamsStore {
    backend: PgBackend,
}

impl PgDreamsStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = dreams)]
struct NewDream {
    workspace_id: String,
    dream_id: String,
    name: String,
    status: String,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = dream_runs)]
struct NewRun {
    workspace_id: String,
    dream_id: String,
    run_id: String,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[async_trait]
impl DreamsStore for PgDreamsStore {
    async fn insert(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(dreams::table)
            .values(NewDream {
                workspace_id: ws.0.clone(),
                dream_id: d.id.as_str().to_owned(),
                name: d.name.clone(),
                status: enum_str(&d.status),
                created_at: d.created_at.as_datetime(),
                doc: to_doc(&d)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Dream, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = dreams::table
            .filter(dreams::workspace_id.eq(&ws.0))
            .filter(dreams::dream_id.eq(id.as_str()))
            .select(dreams::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn replace(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::update(
            dreams::table
                .filter(dreams::workspace_id.eq(&ws.0))
                .filter(dreams::dream_id.eq(d.id.as_str())),
        )
        .set((
            dreams::name.eq(&d.name),
            dreams::status.eq(enum_str(&d.status)),
            dreams::doc.eq(to_doc(&d)?),
        ))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Dream>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = dreams::table
            .filter(dreams::workspace_id.eq(&ws.0))
            .order(dreams::created_at.desc())
            .select(dreams::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn push_run(&self, ws: &WorkspaceId, run: DreamRun) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = dreams::table
            .filter(dreams::workspace_id.eq(&ws.0))
            .filter(dreams::dream_id.eq(run.dream_id.as_str()))
            .select(dreams::dream_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        if exists.is_none() {
            return Err(StoreError::NotFound);
        }
        diesel::insert_into(dream_runs::table)
            .values(NewRun {
                workspace_id: ws.0.clone(),
                dream_id: run.dream_id.as_str().to_owned(),
                run_id: run.id.as_str().to_owned(),
                created_at: run.created_at.as_datetime(),
                doc: to_doc(&run)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn runs(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Vec<DreamRun>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = dream_runs::table
            .filter(dream_runs::workspace_id.eq(&ws.0))
            .filter(dream_runs::dream_id.eq(id.as_str()))
            .order(dream_runs::seq.desc())
            .select(dream_runs::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }
}
