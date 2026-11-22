//! The versions an agent's skills name, read in one statement.

use ::skills::{SkillId, SkillVersion, StoreError, VersionRef};
use diesel::sql_types::{Array, BigInt, Nullable, Text};
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use super::super::skills::version_col;
use crate::{DocRow, PgBackend, diesel_be, parse_doc, pool_be};

/// Each ref's version, the newest one for `latest`, in the refs' order.
const LISTED: &str = "SELECT DISTINCT ON (r.n) v.doc
     FROM unnest($2::text[], $3::bigint[], $4::text[]) WITH ORDINALITY AS r(skill_id, version, id, n)
     JOIN skill_versions v ON v.workspace_id = $1 AND v.skill_id = r.skill_id
       AND (r.version IS NULL OR v.version = r.version) AND (r.id IS NULL OR v.doc->>'id' = r.id)
     ORDER BY r.n, v.version DESC";

pub(super) async fn listed(
    backend: &PgBackend,
    ws: &WorkspaceId,
    refs: &[(SkillId, VersionRef)],
) -> Result<Vec<SkillVersion>, StoreError> {
    let ids: Vec<&str> = refs.iter().map(|(id, _)| id.as_str()).collect();
    let numbers: Vec<Option<i64>> = refs
        .iter()
        .map(|(_, which)| match which {
            VersionRef::Number(n) => Some(version_col(n)),
            VersionRef::Latest | VersionRef::Id(_) => None,
        })
        .collect();
    let version_ids: Vec<Option<&str>> = refs
        .iter()
        .map(|(_, which)| match which {
            VersionRef::Id(v) => Some(v.as_str()),
            VersionRef::Latest | VersionRef::Number(_) => None,
        })
        .collect();
    let mut conn = backend.diesel.get().await.map_err(pool_be)?;
    let rows: Vec<DocRow> = diesel::sql_query(LISTED)
        .bind::<Text, _>(ws.as_str())
        .bind::<Array<Text>, _>(&ids)
        .bind::<Array<Nullable<BigInt>>, _>(&numbers)
        .bind::<Array<Nullable<Text>>, _>(&version_ids)
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
    rows.into_iter().map(|r| parse_doc(r.doc)).collect()
}
