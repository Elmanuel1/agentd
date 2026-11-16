//! The history store on Postgres: one table per tracked table in the schema
//! `history` (V17), which is the one list of tracked tables. Its tables and
//! column types are read once at boot; a change is one insert, decoded by
//! column type from the feed's encoding.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use diesel::sql_types::{Jsonb, Text};
use diesel::QueryableByName;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use serde_json::{Map, Value};
use shared::ports::changes::Position;
use shared::ports::history::{HistoryError, HistoryLog, HistoryOp};

use crate::{diesel_be, pool_be, PgBackend, PgError};

/// How the feed encodes a column's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    /// `jsonb`, sent as JSON text.
    JsonText,
    /// `bytea`, sent as base64.
    Base64,
    /// Every other type, sent as Postgres reads it back.
    Plain,
}

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
            let encoding = match c.data_type.as_str() {
                "jsonb" => Encoding::JsonText,
                "bytea" => Encoding::Base64,
                _ => Encoding::Plain,
            };
            tables.entry(c.table_name).or_default().push((c.column_name, encoding));
        }
        Ok(Self { backend, tables: Arc::new(tables) })
    }
}

/// `row` with each column the feed encodes turned back into what Postgres reads.
fn decoded(
    mut row: Map<String, Value>,
    columns: &[(String, Encoding)],
) -> Result<Map<String, Value>, String> {
    for (name, encoding) in columns {
        let Some(Value::String(text)) = row.get(name) else { continue };
        let value = match encoding {
            Encoding::JsonText => serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?,
            Encoding::Base64 => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|e| format!("{name}: {e}"))?;
                Value::String(format!("\\x{}", hex::encode(bytes)))
            }
            Encoding::Plain => continue,
        };
        row.insert(name.clone(), value);
    }
    Ok(row)
}

/// The text form of a log sequence number, as `pg_lsn` reads it.
fn lsn_text(lsn: u64) -> String {
    format!("{:X}/{:X}", lsn >> 32, lsn & 0xFFFF_FFFF)
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
        let mut conn = self.backend.diesel.write().await.map_err(pool_be::<HistoryError>)?;
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
        const CLEARED: &str = r#"'{"content_sha256": null, "content_size_bytes": null, "path": null}'"#;
        let mut conn = self.backend.diesel.write().await.map_err(pool_be::<HistoryError>)?;
        conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                diesel::sql_query(format!(
                    "UPDATE history.memory_versions SET doc = doc || {CLEARED}
                      WHERE workspace_id = $1 AND store_id = $2 AND version_id = $3"
                ))
                .bind::<Text, _>(workspace)
                .bind::<Text, _>(store)
                .bind::<Text, _>(version)
                .execute(c)
                .await?;
                diesel::sql_query(format!(
                    "UPDATE history.memories SET path = NULL, doc = doc || {CLEARED}
                      WHERE workspace_id = $1 AND store_id = $2 AND head_version_id = $3"
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_feed_s_json_text_and_base64_decode_by_column_type() {
        let columns = [("doc".to_owned(), Encoding::JsonText), ("key_digest".to_owned(), Encoding::Base64)];
        let row = json!({"doc": "{\"a\":1}", "key_digest": "AAE=", "name": "n"});
        let row = decoded(row.as_object().unwrap().clone(), &columns).unwrap();
        assert_eq!(Value::Object(row), json!({"doc": {"a": 1}, "key_digest": "\\x0001", "name": "n"}));
        let bad = json!({"doc": "{"}).as_object().unwrap().clone();
        assert!(decoded(bad, &columns).unwrap_err().starts_with("doc:"));
    }

    #[test]
    fn an_lsn_reads_as_pg_lsn_text() {
        assert_eq!(lsn_text((1 << 32) | 0x16B_3748), "1/16B3748");
        assert_eq!(lsn_text(28_418_376), "0/1B1A148");
    }
}
