//! Which of a page's keys no row names.

use diesel::QueryableByName;
use diesel::sql_types::{Array, BigInt, Text};
use diesel_async::RunQueryDsl;
use sessions::store::StoreError;

use crate::entities::blob_keys::Kind;
use crate::{PgBackend, diesel_be, pool_be};

#[derive(QueryableByName)]
struct Unnamed {
    #[diesel(sql_type = BigInt)]
    i: i64,
}

/// The rows that name a key's parts `k.a`, `k.b` and `k.c`.
#[derive(Clone, Copy)]
pub(super) enum Join {
    Credentials,
    Blob(Kind),
}

impl Join {
    fn sql(self) -> &'static str {
        match self {
            Self::Credentials => {
                "LEFT JOIN credentials r ON r.workspace_id = k.a AND r.vault_id = k.b AND r.credential_id = k.c
                 WHERE r.credential_id IS NULL"
            }
            Self::Blob(Kind::File) => {
                "LEFT JOIN files r ON r.workspace_id = k.a AND r.file_id = k.b WHERE r.file_id IS NULL"
            }
            Self::Blob(Kind::SkillVersion) => {
                "LEFT JOIN skill_versions r ON r.workspace_id = k.a AND r.skill_id = k.b AND r.doc->>'id' = k.c
                 WHERE r.skill_id IS NULL"
            }
            Self::Blob(Kind::MemoryVersion) => {
                "LEFT JOIN memory_versions r ON r.workspace_id = k.a AND r.store_id = k.b AND r.version_id = k.c
                 WHERE r.version_id IS NULL"
            }
        }
    }
}

/// The positions in `parts` of the keys no row names.
pub(super) async fn unnamed(
    backend: &PgBackend,
    join: Join,
    parts: Vec<[String; 3]>,
) -> Result<impl Iterator<Item = usize>, StoreError> {
    let (mut a, mut b, mut c) = (vec![], vec![], vec![]);
    for [pa, pb, pc] in parts {
        a.push(pa);
        b.push(pb);
        c.push(pc);
    }
    let sql = format!(
        "SELECT k.i - 1 AS i FROM unnest($1::text[], $2::text[], $3::text[]) WITH ORDINALITY AS k(a, b, c, i) {}",
        join.sql()
    );
    let mut conn = backend.diesel.get().await.map_err(pool_be)?;
    let rows: Vec<Unnamed> = match a.is_empty() {
        true => vec![],
        false => diesel::sql_query(sql)
            .bind::<Array<Text>, _>(a)
            .bind::<Array<Text>, _>(b)
            .bind::<Array<Text>, _>(c)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?,
    };
    Ok(rows.into_iter().filter_map(|r| usize::try_from(r.i).ok()))
}
