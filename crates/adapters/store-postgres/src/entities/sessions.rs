//! `SessionsStore` on Postgres via Diesel: the session rows.

mod archived;

use ::sessions::store::{Order, SessionFilter, SessionsStore, StoreError};
use ::sessions::types::{Session, SessionId, SessionStatus};
use ::work::store::Reach;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::sql_types::{Array, BigInt, Bool, Jsonb, Nullable, Text, Timestamptz};
use diesel::{BoolExpressionMethods, ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::{Pagination, Timestamp, WorkspaceId};

use super::session_events::lock;
use super::work_items::stop::stop;
use crate::schema::sessions;
use crate::{diesel_be, enum_str, parse_doc, pool_be, to_doc, DocRow, PgBackend};

/// Postgres-backed sessions store (Diesel).
#[derive(Debug, Clone)]
pub struct PgSessionsStore {
    backend: PgBackend,
}

impl PgSessionsStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = sessions)]
struct NewSession {
    workspace_id: String,
    session_id: String,
    status: String,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[async_trait]
impl SessionsStore for PgSessionsStore {
    async fn insert(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        diesel::insert_into(sessions::table)
            .values(NewSession {
                workspace_id: ws.as_str().to_owned(),
                session_id: s.id.as_str().to_owned(),
                status: enum_str(&s.status),
                created_at: s.created_at.as_datetime(),
                doc: to_doc(&s)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = sessions::table
            .filter(sessions::workspace_id.eq(ws.as_str()))
            .filter(sessions::session_id.eq(id.as_str()))
            .select(sessions::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn replace(&self, ws: &WorkspaceId, read: &Session, s: Session) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let doc = to_doc(&s)?;
        // The row is locked and compared as a Session, not as raw JSON: a doc
        // written before a field existed re-serializes differently but holds
        // the same session.
        let outcome = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let stored: Option<serde_json::Value> = sessions::table
                        .filter(sessions::workspace_id.eq(ws.as_str()))
                        .filter(sessions::session_id.eq(s.id.as_str()))
                        .select(sessions::doc)
                        .for_update()
                        .first(c)
                        .await
                        .optional()?;
                    let Some(stored) = stored else {
                        return Ok(Err(StoreError::NotFound));
                    };
                    if serde_json::from_value::<Session>(stored).ok().as_ref() != Some(read) {
                        return Ok(Err(StoreError::Conflict));
                    }
                    diesel::update(
                        sessions::table
                            .filter(sessions::workspace_id.eq(ws.as_str()))
                            .filter(sessions::session_id.eq(s.id.as_str())),
                    )
                    .set((sessions::status.eq(enum_str(&s.status)), sessions::doc.eq(doc)))
                    .execute(c)
                    .await?;
                    Ok(Ok(()))
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?;
        outcome
    }

    async fn delete(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let running = enum_str(&SessionStatus::Running);
        let doc = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    // An append in flight commits first; a later one finds no session.
                    lock(c, id).await?;
                    let key =
                        sessions::workspace_id.eq(ws.as_str()).and(sessions::session_id.eq(id.as_str()));
                    let doc: Option<serde_json::Value> =
                        diesel::delete(sessions::table.filter(key).filter(sessions::status.ne(running)))
                            .returning(sessions::doc)
                            .get_result(c)
                            .await
                            .optional()?;
                    let Some(doc) = doc else {
                        let status: Option<String> =
                            sessions::table.filter(key).select(sessions::status).first(c).await.optional()?;
                        return Ok(Err(status.map_or(StoreError::NotFound, |_| StoreError::Conflict)));
                    };
                    stop(c, ws, id.as_str(), &Reach::default(), false, Timestamp::now()).await?;
                    Ok(Ok(doc))
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)??;
        parse_doc(doc)
    }

    async fn list(
        &self,
        ws: &WorkspaceId,
        filter: &SessionFilter,
        page: &Pagination,
    ) -> Result<Vec<Session>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let c = &filter.created;
        let statuses: Vec<String> = filter.statuses.iter().map(enum_str).collect();
        let memory = filter
            .memory_store_id
            .as_ref()
            .map(|id| serde_json::json!([{"type": "memory_store", "memory_store_id": id}]));
        let order = if filter.order == Order::Asc { "ASC" } else { "DESC" };
        let rows: Vec<DocRow> = diesel::sql_query(format!(
            "SELECT doc FROM sessions
             WHERE workspace_id = $1
               AND ($2 OR doc->>'archived_at' IS NULL)
               AND (cardinality($3::text[]) = 0 OR status = ANY($3))
               AND ($4::text IS NULL OR doc->'agent'->>'id' = $4)
               AND ($5::bigint IS NULL OR (doc->'agent'->>'version')::bigint = $5)
               AND ($6::text IS NULL OR doc->>'deployment_id' = $6)
               AND ($7::jsonb IS NULL OR doc->'resources' @> $7)
               AND ($8::timestamptz IS NULL OR created_at > $8)
               AND ($9::timestamptz IS NULL OR created_at >= $9)
               AND ($10::timestamptz IS NULL OR created_at < $10)
               AND ($11::timestamptz IS NULL OR created_at <= $11)
             ORDER BY created_at {order}, session_id {order}
             LIMIT $12 OFFSET $13"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Bool, _>(filter.include_archived)
        .bind::<Array<Text>, _>(statuses)
        .bind::<Nullable<Text>, _>(filter.agent_id.as_deref())
        .bind::<Nullable<BigInt>, _>(filter.agent_version.map(|v| v as i64))
        .bind::<Nullable<Text>, _>(filter.deployment_id.as_deref())
        .bind::<Nullable<Jsonb>, _>(memory)
        .bind::<Nullable<Timestamptz>, _>(c.gt.map(|t| t.as_datetime()))
        .bind::<Nullable<Timestamptz>, _>(c.gte.map(|t| t.as_datetime()))
        .bind::<Nullable<Timestamptz>, _>(c.lt.map(|t| t.as_datetime()))
        .bind::<Nullable<Timestamptz>, _>(c.lte.map(|t| t.as_datetime()))
        .bind::<BigInt, _>(page.fetch_limit())
        .bind::<BigInt, _>(page.sql_offset())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        rows.into_iter().map(|r| parse_doc(r.doc)).collect()
    }
}
