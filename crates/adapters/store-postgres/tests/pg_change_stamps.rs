//! Every write of a vault, credential, workspace, environment or organization row sets its
//! `updated_at` to the transaction's time and raises its `version` (V31), whatever statement
//! makes it. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod org;

use diesel::sql_types::{BigInt, Bool, Text};
use diesel_async::RunQueryDsl;
use shared::Timestamp;
use store_postgres::PgBackend;

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

#[derive(diesel::QueryableByName, Debug, PartialEq, Eq)]
struct Stamp {
    #[diesel(sql_type = BigInt)]
    version: i64,
    /// Whether `updated_at` is the writing transaction's `now()`.
    #[diesel(sql_type = Bool)]
    now: bool,
    #[diesel(sql_type = Text)]
    doc: String,
}

/// Run `sql` (`$1` = `id`, `$2` = `scope`, the row's workspace or org) and read the row it returns.
async fn write(pg: &PgBackend, sql: &str, id: &str, scope: &str) -> Stamp {
    let mut conn = pg.diesel.write().await.unwrap();
    diesel::sql_query(sql).bind::<Text, _>(id).bind::<Text, _>(scope).get_result(&mut *conn).await.unwrap()
}

const RETURNING: &str = "RETURNING version, updated_at = now() AS now, doc::text AS doc";
const WIRE: &str = "{\"updated_at\": \"wire\"}";

/// Each table's insert of one row and an update that leaves its `doc` alone.
fn statements(table: &str) -> (String, String) {
    let (columns, values, key) = match table {
        "vaults" => {
            ("workspace_id, vault_id, display_name", "$2, $1, 'v'", "workspace_id = $2 AND vault_id = $1")
        }
        "credentials" => (
            "workspace_id, vault_id, credential_id, display_name, has_secret",
            "$2, 'vlt_stamp', $1, 'c', false",
            "workspace_id = $2 AND credential_id = $1",
        ),
        "workspaces" => ("org_id, workspace_id", "$2, $1", "org_id = $2 AND workspace_id = $1"),
        "environments" => (
            "org_id, environment_id, name, state",
            "$2, $1, 'e', 'active'",
            "org_id = $2 AND environment_id = $1",
        ),
        _ => unreachable!(),
    };
    let insert = format!(
        "INSERT INTO {table} ({columns}, created_at, doc) VALUES ({values}, now(), '{WIRE}') {RETURNING}"
    );
    let update = format!("UPDATE {table} SET created_at = created_at WHERE {key} {RETURNING}");
    (insert, update)
}

#[tokio::test]
async fn an_insert_starts_at_version_one_and_each_update_raises_it() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let t = Timestamp::now().as_datetime().timestamp_nanos_opt().unwrap();
    let org = format!("org_stamp{t}");
    org::ensure(&pg, &org).await;
    for table in ["vaults", "credentials", "workspaces", "environments"] {
        let id = format!("id_{table}{t}");
        let scope = match table {
            "vaults" | "credentials" => format!("wrkspc_stamp{t}"),
            _ => org.clone(),
        };
        let (insert, update) = statements(table);
        let at = |version| Stamp { version, now: true, doc: WIRE.to_owned() };
        assert_eq!(write(&pg, &insert, &id, &scope).await, at(1), "{table}");
        assert_eq!(write(&pg, &update, &id, &scope).await, at(2), "{table}");
        assert_eq!(write(&pg, &update, &id, &scope).await, at(3), "{table}");
    }
}

#[tokio::test]
async fn an_organization_row_is_stamped_too() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let org = format!("org_stampo{}", Timestamp::now().as_datetime().timestamp_nanos_opt().unwrap());
    let returning = "RETURNING version, updated_at = now() AS now, $2 AS doc";
    let insert = format!("INSERT INTO organizations (org_id) VALUES ($1) {returning}");
    let update = format!("UPDATE organizations SET created_at = created_at WHERE org_id = $1 {returning}");
    let at = |version| Stamp { version, now: true, doc: String::new() };
    assert_eq!(write(&pg, &insert, &org, "").await, at(1));
    assert_eq!(write(&pg, &update, &org, "").await, at(2));
}
