# PostgreSQL persistence (agentd-store-postgres) — the default

Chosen because agentd is **dashboard-first**. Everything a UI needs —
list, filter, sort, search, paginate, count — is plain SQL, with no second
search copy to build or keep in sync. agentd stays **stateless across
replicas** (all state in Postgres), and multi-replica **exactly-once** is a
native `INSERT … ON CONFLICT DO NOTHING`.

## Versions (Flyway-compatible)

| Component | Version | Note |
|---|---|---|
| PostgreSQL | **16** | Flyway 11 supports 9.5→latest; 16 is the safe LTS-grade pick |
| Flyway | **11** (11.20.3 tested) | manages `migrations/V*.sql`; 12.x also works |
| sqlx | 0.8 | runtime queries (no compile-time DB needed) |

## Schema — hybrid (queryable columns + `jsonb doc`)

Each table keeps the exact wire object in a `doc jsonb` column (perfect
fidelity, no lossy mapping) **and** lifts the fields a dashboard touches into
real, indexed columns:

```sql
create table agents (
  workspace_id text, agent_id text, version bigint,
  name text, archived_at timestamptz, created_at timestamptz,
  doc jsonb, primary key (workspace_id, agent_id, version));
create index agents_ws_created_idx on agents (workspace_id, created_at desc);
create index agents_ws_name_idx    on agents (workspace_id, name);
```

Dashboard queries are then trivial:

| Dashboard need | SQL |
|---|---|
| list latest of each agent | `select distinct on (agent_id) doc … order by agent_id, version desc` |
| newest first | indexed `order by created_at desc` |
| only active deployments | `where status = 'active'` (indexed) |
| failed runs only | `where has_error = true` |
| count tile | `select count(*) …` (indexed) |

## Migrations are Flyway-managed

The crate **never creates tables** — it only uses the migrated schema, so
schema changes go through reviewed `V*.sql` migrations (the tosspaper
convention). Apply them with real Flyway:

```sh
docker run --rm --network host \
  -v "$PWD/crates/agentd-store-postgres/migrations:/flyway/sql" \
  flyway/flyway:11-alpine \
  -url=jdbc:postgresql://localhost:5432/agentd -user=agentd -password=agentd \
  -locations=filesystem:/flyway/sql migrate
```

`docker compose up` does this automatically (the `flyway` service runs
`migrate` and exits; `agentd` waits for it via
`service_completed_successfully`).

## Run the integration tests

Hermetic `cargo test` skips them; against a live migrated DB:

```sh
AGENTD_PG_TESTS=1 DATABASE_URL=postgres://agentd:agentd@localhost:5432/agentd \
  cargo test -p agentd-store-postgres --test pg_integration
```

`dashboard_queries_are_native_sql` proves the list/filter/count value;
`pg_claim_fire_exactly_once_across_concurrent_callers` races 8 callers at one
`(deployment, scheduled_at)` and asserts exactly one wins the `ON CONFLICT`.

## Coverage

Implemented + tested against live Postgres: **agents** (versioned, `DISTINCT
ON` list), **deployments** (+ runs + claim), **files** (metadata in Postgres,
bytes in object storage via the shared `agentd-blobs` `BlobStore` — never in
the DB). Remaining entities reuse the same hybrid pattern — one module + a
`V2__*.sql` (or extend `V1`) each.

## Postgres vs DynamoDB (agentd-store-aws)

Both are behind the same store traits — switch with `AGENTD_STORE`.

| | Postgres (default) | DynamoDB + S3 |
|---|---|---|
| Dashboards (list/filter/sort/search/count) | native SQL | needs a separate search copy |
| Ops | you run/scale the DB (or Aurora Serverless) | fully managed, auto-scales |
| Best for | dashboard-first products (agentd) | extreme scale / write-heavy, simple lookups |
