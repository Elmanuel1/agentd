//! The tracked tables: those whose writes and deletes reach the change feed with their writer, by
//! their `<table>_history_stamp` trigger.

use diesel::QueryableByName;
use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;

use crate::{PgBackend, PgError};

#[derive(QueryableByName)]
struct Table {
    #[diesel(sql_type = Text)]
    name: String,
}

pub async fn tracked_tables(pg: &PgBackend) -> Result<Vec<String>, PgError> {
    let mut conn = pg.diesel.get().await.map_err(|e| PgError::Query(e.to_string()))?;
    let tables: Vec<Table> = diesel::sql_query(
        "SELECT c.relname::text AS name FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid
         WHERE t.tgname = c.relname || '_history_stamp' ORDER BY 1",
    )
    .load(&mut conn)
    .await
    .map_err(|e| PgError::Query(e.to_string()))?;
    Ok(tables.into_iter().map(|t| t.name).collect())
}
