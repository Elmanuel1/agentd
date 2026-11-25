//! Every org-scoped row names an existing organization; tests make theirs first.

use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;
use store_postgres::PgBackend;

/// Make the organization `org` unless it exists.
pub async fn ensure(pg: &PgBackend, org: &str) {
    let mut conn = pg.diesel.write().await.unwrap();
    diesel::sql_query("INSERT INTO organizations (org_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind::<Text, _>(org)
        .execute(&mut *conn)
        .await
        .unwrap();
}
