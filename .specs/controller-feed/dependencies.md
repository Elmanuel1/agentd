# Controller feed: dependencies

Each fact the design rests on, with its evidence. Every row was read in the code on `main` (192c70a) or in the
Postgres documentation.

## agentd today

| # | Fact | Status | Evidence |
|---|---|---|---|
| A1 | Every environment read and write goes through the `environments` view: insert, get, replace, delete and list in `PgEnvironmentsStore`. No route, response field or store method reads `environment_revisions`, `revision`, `superseded_by` or `deleted_by`; only V7, V8 and the test `pg_revisions.rs` do. | VERIFIED | `store-postgres/src/entities/environments.rs`; `resources/environments/src/{store,service,routes,types}.rs` |
| A2 | Diesel's `environments` table already has the six columns a plain table keeps (`org_id`, `environment_id`, `name`, `state`, `created_at`, `doc`). | VERIFIED | `store-postgres/src/schema.rs:66` |
| A3 | agentd keeps no organization record. An org id is the free-text `--org` of `agentd keys`, `admin-keys` and `environment-keys create`, copied into the key row; an admin key resolves to `AdminContext { org }` from `admin_keys.org_id`. | VERIFIED | `api/src/cli/command.rs:45`; `resources/api-keys/src/admin.rs:38,70`; `resources/organizations/src/lib.rs:5` |
| A4 | The tables with `org_id` are `environments` (V2), `api_keys` (V5), `workspaces` (V6), `work_items` and `environment_keys` (V9) and `admin_keys` (V15). Every other resource table is keyed by `workspace_id` with no `org_id`. | VERIFIED | migrations V1, V2, V5, V6, V9, V15, V20 |
| A5 | No migration declares a foreign key. | VERIFIED | `references` in `migrations/`: 0 hits |
| A6 | A workspace key can name a workspace with no `workspaces` row: CI creates `--workspace wrkspc_sdk` with no workspace. | VERIFIED | `.github/workflows/ci.yml:221`; `api/src/cli.rs:43` |
| A7 | No table has an `updated_at` column. Vaults and credentials keep `updated_at` in `doc`, set in Rust; workspaces have none. | VERIFIED | `updated_at` in `migrations/`: 1 hit (`credential_updated_at`, V9:83) |
| A8 | Credentials are inserted or updated by 5 statements (3 raw SQL: the claim, the claimed write, the vault archive cascade), vaults by 3, workspaces by 2. Rotation writes `vault_secrets` and then the credentials row; `agentd vault-secrets reseal` writes `vault_secrets` only. | VERIFIED | `store-postgres/src/entities/{vaults,vault_credentials,workspaces}*`; `resources/vaults/src/rotation.rs:55` |
| A9 | A vault delete removes its credentials in its transaction and purges their secrets after the commit; a file delete removes its blob. Skill-version bundles and memory-version contents also live in the blob store. A deployment archive writes `ScheduleChange::Delete` into `schedule_outbox` in its transaction; agentd has no deployment delete. | VERIFIED | `resources/vaults/src/cascade.rs:22`; `store-postgres/src/entities/vaults/archive.rs:115`; `resources/files/src/service.rs:170`; `store-postgres/src/entities/skills.rs:80`, `skill_versions.rs:74`, `memory/versions.rs:29`; `resources/deployments/src/service.rs:183` |
| A10 | The tracked tables are the tables with a `_history_stamp` trigger: 18 in V16. | VERIFIED | `migrations/V16__history_capture.sql` |
| A11 | Debezium's include list is one line, the outbox tables and the tracked tables; a test fails when it misses a table with a `_history_stamp` trigger. | VERIFIED | `docker/debezium/application.properties:37`; `store-postgres/tests/debezium_include.rs` |

## Postgres

| # | Fact | Status | Evidence |
|---|---|---|---|
| P1 | Debezium's Postgres connector captures tables through a publication; a view is not published and has no change stream. | VERIFIED | Postgres docs, CREATE PUBLICATION ("Only persistent base tables and partitioned tables can be part of a publication") |
| P2 | A row a foreign key's `on delete cascade` removes fires that table's row-level `before delete` triggers. | VERIFIED | Postgres docs, CREATE TRIGGER notes: referential actions run as ordinary SQL statements and fire triggers |
| P3 | `now()` is the start time of the transaction, the same for every row it writes. | VERIFIED | Postgres docs, Current Date/Time |
