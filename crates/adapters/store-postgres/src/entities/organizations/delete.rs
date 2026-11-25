//! An organization delete, in one transaction: its workspace-scoped rows explicitly, a schedule
//! delete in the outbox for each deployment not archived, then the organization row, whose foreign
//! keys cascade to its org-scoped rows.

use ::organizations::registry::{Removed, StoreError};
use diesel::QueryableByName;
use diesel::sql_types::{Array, Bool, Text};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::OrgId;
use shared::ports::secrets::SecretKey;
use shared::ports::{FireTarget, ScheduleChange};

use crate::diesel_be;
use crate::entities::blob_keys;
use crate::entities::schedules::outbox;

/// The workspace-scoped tables with nothing outside Postgres to remove.
const PLAIN: [&str; 16] = [
    "agents",
    "deployment_runs",
    "deployments",
    "vaults",
    "skills",
    "memory_stores",
    "memories",
    "session_events",
    "sessions",
    "dream_runs",
    "dreams",
    "loop_steps",
    "mcp_connections",
    "schedules",
    "fire_claims",
    "vault_secrets",
];

#[derive(QueryableByName)]
struct One {
    #[diesel(sql_type = Text)]
    a: String,
}

#[derive(QueryableByName)]
struct Three {
    #[diesel(sql_type = Text)]
    a: String,
    #[diesel(sql_type = Text)]
    b: String,
    #[diesel(sql_type = Text)]
    c: String,
}

#[derive(QueryableByName)]
struct MemoryVersion {
    #[diesel(sql_type = Text)]
    a: String,
    #[diesel(sql_type = Text)]
    b: String,
    #[diesel(sql_type = Text)]
    c: String,
    #[diesel(sql_type = Bool)]
    has_blob: bool,
}

pub(super) async fn organization(conn: &mut AsyncPgConnection, org: &OrgId) -> Result<Removed, StoreError> {
    conn.transaction::<_, diesel::result::Error, _>(async move |c| {
        // Precondition: the row lock blocks a key insert for this org (its foreign key check takes
        // a key-share lock) until the delete commits, so no workspace is named after it is read.
        let locked: Vec<One> =
            diesel::sql_query("SELECT org_id AS a FROM organizations WHERE org_id = $1 FOR UPDATE")
                .bind::<Text, _>(org.as_str())
                .load(c)
                .await?;
        if locked.is_empty() {
            return Ok(Err(StoreError::NotFound(org.as_str().to_owned())));
        }
        let workspaces: Vec<String> = diesel::sql_query(
            "SELECT workspace_id AS a FROM workspaces WHERE org_id = $1
             UNION SELECT workspace_id FROM api_keys WHERE org_id = $1
             UNION SELECT workspace_id FROM environment_keys WHERE org_id = $1",
        )
        .bind::<Text, _>(org.as_str())
        .load::<One>(c)
        .await?
        .into_iter()
        .map(|r| r.a)
        .collect();
        let removed = rows(c, workspaces).await?;
        diesel::sql_query("DELETE FROM organizations WHERE org_id = $1")
            .bind::<Text, _>(org.as_str())
            .execute(c)
            .await?;
        Ok(Ok(removed))
    })
    .await
    .map_err(diesel_be)?
}

/// Delete every workspace-scoped row of `workspaces`, keeping what lives outside Postgres.
async fn rows(c: &mut AsyncPgConnection, workspaces: Vec<String>) -> Result<Removed, diesel::result::Error> {
    let deployments = ids(
        c,
        "SELECT deployment_id AS a FROM deployments
                               WHERE workspace_id = ANY($1) AND status <> 'archived'",
        &workspaces,
    )
    .await?;
    let schedule_deletes: Vec<ScheduleChange> = deployments
        .into_iter()
        .map(|id| ScheduleChange::Delete { target: FireTarget::new(deployments::fire::KIND, id) })
        .collect();
    outbox::record(c, &schedule_deletes).await?;
    let dreams = ids(
        c,
        "SELECT dream_id AS a FROM dreams
                          WHERE workspace_id = ANY($1) AND status <> 'archived'",
        &workspaces,
    )
    .await?;
    let credentials: Vec<Three> = in_workspaces(
        c,
        "DELETE FROM credentials WHERE workspace_id = ANY($1)
         RETURNING workspace_id AS a, vault_id AS b, credential_id AS c",
        &workspaces,
    )
    .await?;
    let files: Vec<Three> = in_workspaces(
        c,
        "DELETE FROM files WHERE workspace_id = ANY($1) RETURNING workspace_id AS a, file_id AS b, ''::text AS c",
        &workspaces,
    )
    .await?;
    let skill_versions: Vec<Three> = in_workspaces(
        c,
        "DELETE FROM skill_versions WHERE workspace_id = ANY($1)
         RETURNING workspace_id AS a, skill_id AS b, doc->>'id' AS c",
        &workspaces,
    )
    .await?;
    let memory_versions: Vec<MemoryVersion> = in_workspaces(
        c,
        "DELETE FROM memory_versions WHERE workspace_id = ANY($1)
         RETURNING workspace_id AS a, store_id AS b, version_id AS c, has_blob",
        &workspaces,
    )
    .await?;
    let mut counts = vec![
        ("credentials", len(&credentials)),
        ("files", len(&files)),
        ("skill_versions", len(&skill_versions)),
        ("memory_versions", len(&memory_versions)),
    ];
    for table in PLAIN {
        let n = diesel::sql_query(format!("DELETE FROM {table} WHERE workspace_id = ANY($1)"))
            .bind::<Array<Text>, _>(&workspaces)
            .execute(c)
            .await?;
        counts.push((table, u64::try_from(n).unwrap_or(u64::MAX)));
    }
    let secrets = credentials.iter().filter_map(|r| SecretKey::new(&r.a, &r.b, &r.c).ok()).collect();
    let blobs = files
        .iter()
        .map(|r| blob_keys::file(&r.a, &r.b))
        .chain(skill_versions.iter().map(|r| blob_keys::skill_version(&r.a, &r.b, &r.c)))
        .chain(
            memory_versions
                .iter()
                .filter(|r| r.has_blob)
                .map(|r| blob_keys::memory_version(&r.a, &r.b, &r.c)),
        )
        .collect();
    let dreams = dreams.into_iter().map(|id| FireTarget::new(dreams::fire::KIND, id)).collect();
    Ok(Removed { workspaces, rows: counts, secrets, blobs, dreams })
}

async fn ids(
    c: &mut AsyncPgConnection,
    sql: &str,
    workspaces: &[String],
) -> Result<Vec<String>, diesel::result::Error> {
    let rows: Vec<One> = in_workspaces(c, sql, workspaces).await?;
    Ok(rows.into_iter().map(|r| r.a).collect())
}

async fn in_workspaces<T: QueryableByName<diesel::pg::Pg> + 'static>(
    c: &mut AsyncPgConnection,
    sql: &str,
    workspaces: &[String],
) -> Result<Vec<T>, diesel::result::Error> {
    diesel::sql_query(sql).bind::<Array<Text>, _>(workspaces).load(c).await
}

fn len<T>(rows: &[T]) -> u64 {
    u64::try_from(rows.len()).unwrap_or(u64::MAX)
}
