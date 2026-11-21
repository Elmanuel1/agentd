//! A delivery's row is written in the commit of the change it reports, so it
//! lands exactly when its change does (H-1).

use ::sessions::types::SessionId;
use diesel::OptionalExtension;
use diesel::sql_types::{Array, Text};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::WorkspaceId;
use webhooks::Delivery;

pub(crate) async fn insert_deliveries(
    c: &mut AsyncPgConnection,
    deliveries: &[Delivery],
) -> Result<(), diesel::result::Error> {
    if deliveries.is_empty() {
        return Ok(());
    }
    let col = |f: fn(&Delivery) -> &str| deliveries.iter().map(|r| f(r).to_owned()).collect::<Vec<_>>();
    diesel::sql_query(
        "INSERT INTO webhook_deliveries (webhook_id, endpoint_key, endpoint_url, body, next_attempt_at) \
         SELECT w, k, u, b, now() FROM unnest($1::text[], $2::text[], $3::text[], $4::text[]) AS d(w, k, u, b)",
    )
    .bind::<Array<Text>, _>(col(|r| &r.webhook_id))
    .bind::<Array<Text>, _>(col(|r| &r.endpoint_key))
    .bind::<Array<Text>, _>(col(|r| &r.endpoint_url))
    .bind::<Array<Text>, _>(col(|r| &r.body))
    .execute(c)
    .await
    .map(drop)
}

#[derive(diesel::QueryableByName)]
struct Org {
    #[diesel(sql_type = Text)]
    org_id: String,
}

/// The deliveries `build` makes for a session's status change, in its
/// commit: the organization read from the session's work item, which every
/// session has.
pub(crate) async fn status_changed(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
    build: impl FnOnce(&str) -> Vec<Delivery>,
) -> Result<(), diesel::result::Error> {
    let org: Option<Org> =
        diesel::sql_query("SELECT org_id FROM work_items WHERE workspace_id = $1 AND session_id = $2")
            .bind::<Text, _>(ws.as_str())
            .bind::<Text, _>(id.as_str())
            .get_result(c)
            .await
            .optional()?;
    let Some(org) = org else { return Ok(()) };
    insert_deliveries(c, &build(&org.org_id)).await
}
