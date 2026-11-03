//! `WorkspacesStore` on Postgres via Diesel — org-scoped.

use ::organizations::workspaces::{StoreError, Workspace, WorkspacesStore};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{BoolExpressionMethods, ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::pooled_connection::deadpool::Object;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::{IdCursor, OrgId};

use crate::schema::workspaces;
use crate::{diesel_be, parse_doc, pool_be, to_doc, PgBackend};

/// Postgres-backed workspaces store (Diesel).
#[derive(Debug, Clone)]
pub struct PgWorkspacesStore {
    backend: PgBackend,
}

impl PgWorkspacesStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = workspaces)]
struct NewWorkspace<'a> {
    org_id: &'a str,
    workspace_id: &'a str,
    archived_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

/// A cursor id's place in the list order; `None` for an id the org lacks,
/// which bounds nothing.
async fn sort_key(
    conn: &mut Object<AsyncPgConnection>,
    org: &OrgId,
    id: &str,
) -> Result<Option<(DateTime<Utc>, String)>, StoreError> {
    workspaces::table
        .filter(workspaces::org_id.eq(org.as_str()))
        .filter(workspaces::workspace_id.eq(id))
        .select((workspaces::created_at, workspaces::workspace_id))
        .first(conn)
        .await
        .optional()
        .map_err(diesel_be)
}

#[async_trait]
impl WorkspacesStore for PgWorkspacesStore {
    async fn insert(&self, org: &OrgId, workspace: &Workspace) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(workspaces::table)
            .values(NewWorkspace {
                org_id: org.as_str(),
                workspace_id: &workspace.id,
                archived_at: workspace.archived_at.map(|t| t.as_datetime()),
                created_at: workspace.created_at.as_datetime(),
                doc: to_doc(workspace)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(&self, org: &OrgId, id: &str) -> Result<Workspace, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = workspaces::table
            .filter(workspaces::org_id.eq(org.as_str()))
            .filter(workspaces::workspace_id.eq(id))
            .select(workspaces::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn replace(&self, org: &OrgId, workspace: &Workspace) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc = to_doc(workspace)?;
        let affected = diesel::update(
            workspaces::table
                .filter(workspaces::org_id.eq(org.as_str()))
                .filter(workspaces::workspace_id.eq(&workspace.id)),
        )
        .set((
            workspaces::archived_at.eq(workspace.archived_at.map(|t| t.as_datetime())),
            workspaces::doc.eq(doc),
        ))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn list(
        &self,
        org: &OrgId,
        include_archived: bool,
        cursor: &IdCursor,
    ) -> Result<(Vec<Workspace>, bool), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let after = match &cursor.after_id {
            Some(id) => sort_key(&mut conn, org, id).await?,
            None => None,
        };
        let before = match &cursor.before_id {
            Some(id) => sort_key(&mut conn, org, id).await?,
            None => None,
        };
        let mut q = workspaces::table.filter(workspaces::org_id.eq(org.as_str())).into_boxed();
        if !include_archived {
            q = q.filter(workspaces::archived_at.is_null());
        }
        // Keyset on (created_at DESC, workspace_id DESC): after a cursor is a
        // smaller key, before it a larger one.
        if let Some((at, id)) = after {
            q = q.filter(
                workspaces::created_at
                    .lt(at)
                    .or(workspaces::created_at.eq(at).and(workspaces::workspace_id.lt(id))),
            );
        }
        let backwards = before.is_some() && cursor.after_id.is_none();
        if let Some((at, id)) = before {
            q = q.filter(
                workspaces::created_at
                    .gt(at)
                    .or(workspaces::created_at.eq(at).and(workspaces::workspace_id.gt(id))),
            );
        }
        // A page before a cursor is the rows nearest it, so it is read oldest
        // first and turned back to newest first.
        q = if backwards {
            q.order((workspaces::created_at.asc(), workspaces::workspace_id.asc()))
        } else {
            q.order((workspaces::created_at.desc(), workspaces::workspace_id.desc()))
        };
        let docs: Vec<serde_json::Value> =
            q.select(workspaces::doc).limit(cursor.fetch_limit()).load(&mut conn).await.map_err(diesel_be)?;
        let mut rows = docs.into_iter().map(parse_doc).collect::<Result<Vec<Workspace>, StoreError>>()?;
        let has_more = rows.len() > cursor.limit;
        rows.truncate(cursor.limit);
        if backwards {
            rows.reverse();
        }
        Ok((rows, has_more))
    }
}
