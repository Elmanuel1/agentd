# Controller feed: decisions

agentd's change feed carries every organization, environment, workspace, vault and credential row, each with the
time of its last write, so a controller outside agentd can rebuild its objects per organization from the feed
alone ([ADR 0001](adr/0001-organizations-and-plain-environments.md)). The first consumer is the
agentd-egress-proxy-controller, which runs one iron-proxy per workspace and cloud environment. Each row names who
decided and when.

## Organizations

| # | Decision | Source |
|---|---|---|
| D1 | agentd keeps one row per organization in a table `organizations`: `org_id` (the key), `created_at`, `updated_at`, and the writer columns `actor` and `request_id`. No name and no other column. | User, 2026-11-23 ("fix 1, 2, 3") |
| D2 | `agentd organization create` is the one way an organization comes to exist. It prints the new id, an `org_` id it generates, or the exact id given with `--id`, so CI, docker-compose, perf and conformance keep `org_sdk`, `org_dev` and `org_perf`. | User, 2026-11-23 ("available via cli. agentd organization create."; "Generate, optional --id") |
| D3 | Every org-scoped row names an existing organization: `workspaces`, `environments`, `admin_keys`, `api_keys`, `environment_keys` and `work_items` each get a foreign key on `org_id` to `organizations`. `agentd keys`, `admin-keys` and `environment-keys create` for an unknown org fail and name the org. | User, 2026-11-23 ("Keys and workspaces must then belong to an existing organization row") |
| D4 | `agentd organization delete <id>` removes the organization with everything it owns, in one transaction: its workspaces and every workspace-scoped row (agents, deployments, files, vaults, credentials, skills, memory, sessions and their events, dreams, the loop and schedule rows), its environments and its admin, API and environment keys. Secrets and file blobs outside Postgres are removed after the commit, as a vault delete and a file delete remove them today. | User, 2026-11-23 ("remove with everything") |
| D5 | The org-scoped tables cascade through their foreign keys (`on delete cascade`). The workspace-scoped tables carry no `org_id`, so the delete removes their rows explicitly in the same transaction, for every workspace the organization owns: its `workspaces` rows and every workspace its API and environment keys name (a workspace key can name a workspace with no row). | Plan audit, 2026-11-23: workspace keys are created for ids with no `workspaces` row (`ci.yml:221`); no table has a foreign key today |
| D6 | Every row the delete removes reaches the feed as a delete, through each tracked table's `history_deleted` trigger; Postgres fires row triggers on rows a foreign key cascade removes. | User, 2026-11-23 (the brief) |
| D7 | No Admin API route creates or deletes an organization. Anthropic's Admin API has none, and agentd adds no endpoint Anthropic lacks; `GET /v1/organizations/me` is unchanged. | Memory `feedback-agentd-match-anthropic`; user, 2026-11-06 ("dont add an endpoint anthropic never added") |
| D8 | An organization delete sends no webhook deliveries for the rows it removes; the organization they would report to is gone. | Plan audit, 2026-11-23: live has no organization delete to match |
| D9 | The migration creates `organizations` empty and backfills nothing from existing key rows: agentd is not in production. | Memory `project-agentd-not-in-production` |

## Change times

| # | Decision | Source |
|---|---|---|
| D10 | `vaults`, `credentials`, `workspaces`, `environments` and `organizations` each have a column `updated_at timestamptz not null`, set on every insert and update, archive and secret rotation included. | User, 2026-11-23 ("fix 1, 2, 3") |
| D11 | One `before insert or update` trigger function, `stamp_updated_at()`, sets `updated_at` to the transaction's `now()`, the shape `history_stamp()` (V16) already uses. A trigger reaches the raw SQL writes too (the credential claim, the vault archive cascade), which a Rust-set column would each have to repeat. The wire `doc.updated_at` keeps its value; the column is the row's write time. | Plan audit, 2026-11-23: 7 credential, 3 vault and 2 workspace write statements, 3 of them raw SQL |

## Environments

| # | Decision | Source |
|---|---|---|
| D12 | Environments are one plain table `environments`, one row per environment, with a real `DELETE`. The `environment_revisions` table, the `environments` view, its `instead of` triggers and `revisions_append_only()` are dropped. | User, 2026-11-23 ("no more revisions. a proper table now. with dlete") |
| D13 | The migration copies each live revision into the new table before dropping `environment_revisions`; past revisions are not kept. | User, 2026-11-23 ("no more revisions"); memory `feedback-no-truncate` |
| D14 | `environments` and `organizations` become tracked tables: writer columns, `replica identity full`, the `history_stamp` and `history_deleted` triggers, a `history.<table>` table, a branch in `history.changes`, and an entry in Debezium's include list (`docker/debezium/application.properties`). The tracked tables are 20. | User, 2026-11-23 (the brief); resource-history D2 |
| D15 | No route, response field or store method changes: every reader already reads the live view. | Plan audit, 2026-11-23 (dependencies A1) |

## Delivery

| # | Decision | Source |
|---|---|---|
| D16 | This spec ships as its own PR; the code ships as one PR of ordered commits under 100 files. | Memory `feedback-agentd-fewer-prs` |

Out of scope: [requirements.md](requirements.md#out-of-scope).
