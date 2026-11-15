//! A session's replace, under its row lock and compared with what the
//! writer read; an archive's also stops the work item and terminates the
//! session in the same commit.

use ::sessions::store::StoreError;
use ::sessions::types::Session;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::WorkspaceId;

use super::PgSessionsStore;
use crate::schema::sessions;
use crate::{diesel_be, enum_str, pool_be, to_doc};

impl PgSessionsStore {
    /// `replace`, with the webhook deliveries `sent` in its commit.
    pub(super) async fn replace_sending(
        &self,
        ws: &WorkspaceId,
        read: &Session,
        s: Session,
        sent: &[webhooks::Delivery],
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let doc = to_doc(&s)?;
        // The row is locked and compared as a Session, not as raw JSON: a doc
        // written before a field existed re-serializes differently but holds
        // the same session.
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
                    if read.archived_at.is_none() && s.archived_at.is_some() {
                        super::terminate::archived(c, ws, read, &s, sent).await?;
                    }
                    Ok(Ok(()))
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?;
        outcome
    }
}
