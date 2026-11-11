//! The archived sessions as event retention walks them.

use ::sessions::retention::{ArchivedCursor, ArchivedSession, ArchivedSessions};
use ::sessions::store::StoreError;
use ::sessions::types::SessionId;
use async_trait::async_trait;
use diesel::sql_types::{BigInt, Text};
use diesel_async::RunQueryDsl;
use shared::{Timestamp, WorkspaceId};

use super::PgSessionsStore;
use crate::{diesel_be, pool_be};

#[derive(diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type = Text)]
    workspace_id: String,
    #[diesel(sql_type = Text)]
    session_id: String,
    #[diesel(sql_type = Text)]
    archived_at: String,
}

#[async_trait]
impl ArchivedSessions for PgSessionsStore {
    async fn archived_between(
        &self,
        after: &ArchivedCursor,
        to: Timestamp,
        limit: usize,
    ) -> Result<Vec<ArchivedSession>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // Keyset on the `sessions_archived_at` index; `archived_at` is in its
        // fixed-width wire form, so text order is time order.
        let rows: Vec<Row> = diesel::sql_query(
            "SELECT workspace_id, session_id, doc->>'archived_at' AS archived_at FROM sessions
             WHERE doc->>'archived_at' IS NOT NULL AND (doc->>'archived_at', session_id) > ($1, $2)
               AND doc->>'archived_at' < $3
             ORDER BY doc->>'archived_at', session_id LIMIT $4",
        )
        .bind::<Text, _>(&after.archived_at)
        .bind::<Text, _>(&after.session_id)
        .bind::<Text, _>(to.to_wire())
        .bind::<BigInt, _>(i64::try_from(limit).unwrap_or(i64::MAX))
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        rows.into_iter()
            .map(|r| {
                let session = SessionId::parse(&r.session_id)
                    .map_err(|e| StoreError::Backend(shared::BackendError::new(e)))?;
                Ok(ArchivedSession {
                    workspace: WorkspaceId::new(r.workspace_id),
                    session,
                    archived_at: r.archived_at,
                })
            })
            .collect()
    }
}
