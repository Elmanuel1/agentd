//! A skill's `latest` pointer, read and moved under the skill row's lock,
//! so version writes on one skill are ordered.

use ::skills::SkillId;
use diesel::sql_types::{Jsonb, Text};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::WorkspaceId;

use crate::schema::skills;

/// The skill's `latest_version` and `latest_version_id`, locking its row;
/// `None` when the skill does not exist.
pub(super) async fn locked_latest(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SkillId,
) -> Result<Option<(String, Option<String>)>, diesel::result::Error> {
    let doc: Option<serde_json::Value> = skills::table
        .filter(skills::workspace_id.eq(ws.as_str()))
        .filter(skills::skill_id.eq(id.as_str()))
        .select(skills::doc)
        .for_update()
        .first(c)
        .await
        .optional()?;
    Ok(doc.map(|d| {
        let text = |k: &str| d.get(k).and_then(serde_json::Value::as_str).map(str::to_owned);
        (text("latest_version").unwrap_or_default(), text("latest_version_id"))
    }))
}

pub(super) async fn set_latest(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SkillId,
    patch: serde_json::Value,
) -> Result<usize, diesel::result::Error> {
    diesel::sql_query("UPDATE skills SET doc = doc || $3 WHERE workspace_id = $1 AND skill_id = $2")
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Jsonb, _>(patch)
        .execute(c)
        .await
}
