# Controller feed: requirements

EARS requirements for the organizations table, the change times and plain environments
([decisions](decisions.md), [ADR 0001](adr/0001-organizations-and-plain-environments.md)).

| Pattern | Form |
|---|---|
| Ubiquitous | The `<system>` shall `<response>`. |
| Event-driven | When `<trigger>`, the `<system>` shall `<response>`. |
| Unwanted | If `<condition>`, then the `<system>` shall `<response>`. |

**Systems:** agentd (the server and its `agentd` CLI); the store (agentd's Postgres schema and migrations); the
change feed (Debezium Server reading the store).

**Terms:** *org-scoped table*: a table with an `org_id` column (dependencies A4). *workspace-scoped table*: a
table keyed by `workspace_id` with no `org_id`. *tracked table*: a table whose changes the history consumer keeps
(resource-history D4). *an organization's workspaces*: the workspace ids of its `workspaces` rows and of its API
and environment keys (D5).

**Build order:** one code PR of ordered commits: CF-20 to CF-24 (environments), then CF-10 to CF-12 (change
times), then CF-1 to CF-8 (organizations), then CF-30 and CF-31 (the feed).

## Organizations

| ID | Requirement |
|---|---|
| CF-1 | The store shall keep one `organizations` row per organization with `org_id` as its key, `created_at`, `updated_at`, `actor` and `request_id`. |
| CF-2 | When `agentd organization create` runs, agentd shall insert an `organizations` row with a new `org_` id and print the id. |
| CF-3 | When `agentd organization create --id <org_id>` runs, agentd shall insert the row with that id and print it, and shall fail naming the id if it already exists. |
| CF-4 | If a `workspaces`, `environments`, `admin_keys`, `api_keys`, `environment_keys` or `work_items` row names an `org_id` with no `organizations` row, then the store shall refuse the write. |
| CF-5 | If `agentd keys`, `admin-keys` or `environment-keys create` names an organization with no row, then agentd shall exit with failure naming the organization, and create no key. |
| CF-6 | When `agentd organization delete <org_id>` runs, agentd shall delete, in one transaction, the `organizations` row, every row of the org-scoped tables with that `org_id`, and every row of the workspace-scoped tables whose workspace is one of the organization's workspaces. |
| CF-7 | When an organization delete commits, agentd shall delete the secrets of the credentials it removed from the secret store and the blobs of the files it removed from the blob store, and shall print what it removed. |
| CF-8 | If `agentd organization delete` names an organization with no row, then agentd shall exit with failure naming it, and delete nothing. |

## Change times

| ID | Requirement |
|---|---|
| CF-10 | The store shall keep a column `updated_at timestamptz not null` on `vaults`, `credentials`, `workspaces`, `environments` and `organizations`. |
| CF-11 | When a row of one of those tables is inserted or updated, by any statement, the store shall set its `updated_at` to the writing transaction's `now()`. |
| CF-12 | agentd shall keep every wire `updated_at` value and response shape as it is today. |

## Environments

| ID | Requirement |
|---|---|
| CF-20 | The store shall keep environments in one plain table `environments`, one row per environment, keyed by `(org_id, environment_id)`, with no `environment_revisions` table, no `environments` view and no revision trigger. |
| CF-21 | When the migration runs, the store shall copy every live environment revision into `environments` with its `org_id`, `environment_id`, `name`, `state`, `created_at` and `doc`. |
| CF-22 | When `DELETE /v1/environments/{id}` succeeds, the store shall remove the environment's row. |
| CF-23 | agentd shall serve every `/v1/environments` route with the same responses as today. |
| CF-24 | The store shall list an organization's environments by `created_at` descending, then `environment_id` descending, from an index on `(org_id, created_at desc, environment_id desc)`. |

## The feed

| ID | Requirement |
|---|---|
| CF-30 | The store shall track `environments` and `organizations` as it tracks the other tracked tables: `actor` and `request_id` columns, `replica identity full`, the `history_stamp` and `history_deleted` triggers, a `history.environments` and a `history.organizations` table, and a branch of each in `history.changes`. |
| CF-31 | The change feed shall capture `public.organizations` and `public.environments`, so it carries each insert, update and delete of every organization, environment, workspace, vault and credential row, the rows an organization delete removes included. |

## Out of scope

- An Admin API route that creates or deletes an organization (D7).
- An organization name, or any column beyond CF-1.
- A workspace delete route or command; workspaces stay archive-only through the API.
- Foreign keys between workspace-scoped tables and `workspaces`.
- Two organizations whose keys name the same workspace id: the workspace-scoped tables carry no `org_id`, so they
  share that workspace's rows today, and an organization delete removes them.
- Stopping a sandbox that is running for a session an organization delete removes.
