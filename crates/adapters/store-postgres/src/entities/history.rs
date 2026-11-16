//! The history store on Postgres: one table per tracked table in the schema
//! `history` (V17), which is the one list of tracked tables. Its tables and
//! column types are read once at boot; a change is one insert, decoded by
//! column type from the feed's encoding.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use diesel::sql_types::{Jsonb, Text};
use diesel::QueryableByName;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use serde_json::Value;
use shared::ports::changes::Position;
use shared::ports::history::{HistoryError, HistoryLog, HistoryOp};

use crate::{diesel_be, pool_be, PgBackend, PgError};

mod decode;

use decode::{decoded, lsn_text, Encoding};

/// Records history into the `history` schema.
#[derive(Clone)]
pub struct PgHistoryLog {
    backend: PgBackend,
    tables: Arc<BTreeMap<String, Vec<(String, Encoding)>>>,
}

impl std::fmt::Debug for PgHistoryLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgHistoryLog").field("tables", &self.tables.len()).finish_non_exhaustive()
    }
}

#[derive(QueryableByName)]
struct HistoryColumn {
    #[diesel(sql_type = Text)]
    table_name: String,
    #[diesel(sql_type = Text)]
    column_name: String,
    #[diesel(sql_type = Text)]
    data_type: String,
}

impl PgHistoryLog {
    /// The history tables of `backend`'s database, read once.
    pub async fn load(backend: PgBackend) -> Result<Self, PgError> {
        let fail = |e: String| PgError::Connect(format!("history tables: {e}"));
        let mut conn = backend.diesel.get().await.map_err(|e| fail(e.to_string()))?;
        let columns: Vec<HistoryColumn> = diesel::sql_query(
            "SELECT c.relname::text AS table_name, a.attname::text AS column_name,
                    format_type(a.atttypid, a.atttypmod) AS data_type
               FROM pg_attribute a
               JOIN pg_class c ON c.oid = a.attrelid
               JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE n.nspname = 'history' AND c.relkind = 'r' AND a.attnum > 0 AND NOT a.attisdropped
              ORDER BY c.relname, a.attnum",
        )
        .load(&mut *conn)
        .await
        .map_err(|e| fail(e.to_string()))?;
        let mut tables: BTreeMap<String, Vec<(String, Encoding)>> = BTreeMap::new();
        for c in columns {
            let encoding = Encoding::of(&c.data_type);
            tables.entry(c.table_name).or_default().push((c.column_name, encoding));
        }
        Ok(Self { backend, tables: Arc::new(tables) })
    }
}

#[async_trait]
impl HistoryLog for PgHistoryLog {
    fn tables(&self) -> Vec<String> {
        self.tables.keys().cloned().collect()
    }

    async fn record(
        &self,
        table: &str,
        op: HistoryOp,
        position: &Position,
        row: &Value,
    ) -> Result<(), HistoryError> {
        let (table, columns) = self
            .tables
            .get_key_value(table)
            .ok_or_else(|| HistoryError::Unrecordable(format!("{table} has no history table")))?;
        let row = row.as_object().cloned().ok_or_else(|| HistoryError::Unrecordable("not a row".into()))?;
        let mut row = match op {
            HistoryOp::Delete => row,
            HistoryOp::Insert | HistoryOp::Update => {
                decoded(row, columns).map_err(HistoryError::Unrecordable)?
            }
        };
        row.insert("lsn".into(), lsn_text(position.lsn).into());
        row.insert("tx_id".into(), position.tx_id.into());
        row.insert("op".into(), op.as_str().into());
        row.insert("committed_at".into(), position.committed_at.to_wire().into());
        // A plain checkout: a history row carries its change's writer, never the connection's.
        let mut conn = self.backend.diesel.get().await.map_err(pool_be::<HistoryError>)?;
        diesel::sql_query(format!(
            "INSERT INTO history.{table} SELECT * FROM jsonb_populate_record(NULL::history.{table}, $1)
             ON CONFLICT DO NOTHING"
        ))
        .bind::<Jsonb, _>(Value::Object(row))
        .execute(&mut *conn)
        .await
        .map(drop)
        .map_err(diesel_be)
    }

    async fn scrub_memory_content(
        &self,
        workspace: &str,
        store: &str,
        version: &str,
    ) -> Result<(), HistoryError> {
        // `r` is the version's redaction as history recorded it; without one, nothing changes.
        const CLEARED: &str = r#"'{"content_sha256": null, "content_size_bytes": null, "path": null}'"#;
        const REDACTED: &str = "r.workspace_id = $1 AND r.store_id = $2 AND r.version_id = $3
                                AND jsonb_typeof(r.doc->'redacted_at') = 'string'";
        let mut conn = self.backend.diesel.get().await.map_err(pool_be::<HistoryError>)?;
        conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                diesel::sql_query(format!(
                    "UPDATE history.memory_versions h SET doc = h.doc || {CLEARED}
                       FROM history.memory_versions r
                      WHERE {REDACTED} AND h.workspace_id = r.workspace_id AND h.store_id = r.store_id
                        AND h.version_id = r.version_id"
                ))
                .bind::<Text, _>(workspace)
                .bind::<Text, _>(store)
                .bind::<Text, _>(version)
                .execute(c)
                .await?;
                diesel::sql_query(format!(
                    "UPDATE history.memories h SET path = NULL, doc = h.doc || {CLEARED}
                       FROM history.memory_versions r
                      WHERE {REDACTED} AND h.workspace_id = r.workspace_id AND h.store_id = r.store_id
                        AND h.memory_id = r.memory_id AND h.head_version_id = r.version_id"
                ))
                .bind::<Text, _>(workspace)
                .bind::<Text, _>(store)
                .bind::<Text, _>(version)
                .execute(c)
                .await
                .map(drop)
            }
            .scope_boxed()
        })
        .await
        .map_err(diesel_be)
    }
}
