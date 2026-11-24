# Controller feed: decisions

agentd's change feed carries every organization, environment, workspace, vault and credential row, each with the
time of its last write, so a controller outside agentd can rebuild its objects per organization from the feed
alone ([ADR 0001](adr/0001-organizations-and-plain-environments.md)). The first consumer is the
agentd-egress-proxy-controller, which runs one iron-proxy per workspace and cloud environment. Each row names who
decided and when.

## Organizations

| # | Decision | Source |
|---|---|---|
| D1 | agentd keeps one row per organization in a table `organizations`: `org_id` (the key), `created_at`, `updated_at`, `version` (D18), and the writer columns `actor` and `request_id`. No name and no other column. | User, 2026-11-23 ("fix 1, 2, 3") |
| D2 | `agentd organization create` is the one way to make a new organization; an operator rollback of a deleted row from its history (resource-history HI-10) brings back its `org_id` and `created_at`, with the rollback's own `actor`, `request_id` and `updated_at`. It prints the new id, an `org_` id it generates, or the exact id given with `--id`, so CI, docker-compose, perf and conformance keep `org_sdk`, `org_dev` and `org_perf`. | User, 2026-11-23 ("available via cli. agentd organization create."; "Generate, optional --id") |
| D3 | Every org-scoped row names an existing organization: `workspaces`, `environments`, `admin_keys`, `api_keys`, `environment_keys` and `work_items` each get a foreign key on `org_id` to `organizations`. `agentd keys`, `admin-keys` and `environment-keys create` for an unknown org fail and name the org. | User, 2026-11-23 ("Keys and workspaces must then belong to an existing organization row") |
| D4 | `agentd organization delete <id>` removes the organization with everything it owns, in one transaction: its workspaces and every workspace-scoped row (agents, deployments, files, vaults, credentials, skills, memory, sessions and their events, dreams, the loop and schedule rows), its environments and its admin, API and environment keys. Each removed deployment not yet archived gets its schedule delete in `schedule_outbox` in the same transaction, as a deployment archive writes it today; an archived one's was written when it was archived. Secrets, file blobs, skill-version bundles and memory-version contents outside Postgres are removed after the commit, as the vault, file, skill and memory deletes remove them today. | User, 2026-11-23 ("remove with everything") |
| D5 | The org-scoped tables cascade through their foreign keys (`on delete cascade`). The workspace-scoped tables carry no `org_id`, so the delete removes their rows explicitly in the same transaction, for every workspace the organization owns: its `workspaces` rows and every workspace its API and environment keys name (a workspace key can name a workspace with no row). | Plan audit, 2026-11-23: workspace keys are created for ids with no `workspaces` row (`ci.yml:221`); no table has a foreign key today |
| D6 | Every row the delete removes reaches the feed as a delete, through each tracked table's `history_deleted` trigger; Postgres fires row triggers on rows a foreign key cascade removes. | User, 2026-11-23 (the brief) |
| D7 | No Admin API route creates or deletes an organization. Anthropic's Admin API has none, and agentd adds no endpoint Anthropic lacks; `GET /v1/organizations/me` is unchanged. | Memory `feedback-agentd-match-anthropic`; user, 2026-11-06 ("dont add an endpoint anthropic never added") |
| D8 | An organization delete queues no webhook deliveries for the rows it removes; the organization they would report to is gone. Deliveries already queued in `webhook_deliveries`, which carries no org or workspace id, go out as queued. | Plan audit, 2026-11-23: live has no organization delete to match |
| D9 | The migration creates `organizations` empty and backfills nothing from existing key rows: agentd is not in production. | Memory `project-agentd-not-in-production` |

## Change times

| # | Decision | Source |
|---|---|---|
| D10 | `vaults`, `credentials`, `workspaces`, `environments` and `organizations` each have a column `updated_at timestamptz not null`, set on every insert and update, archive and secret rotation included. | User, 2026-11-23 ("fix 1, 2, 3") |
| D11 | One `before insert or update` trigger function, `stamp_change()`, sets `updated_at` to the transaction's `now()` and, on an update, sets `version` to the old row's `version` plus one, the shape `history_stamp()` (V16) already uses. A trigger reaches the raw SQL writes too (the credential claim, the vault archive cascade), which a Rust-set column would each have to repeat. The wire `doc.updated_at` keeps its value; the column is the row's write time. It is the transaction's start time, not its commit order, so it is for display and debugging; the controller's check reads `version` (D18). | Plan audit, 2026-11-23: 10 insert and update statements (5 credential, 3 vault, 2 workspace), 3 of them raw SQL |
| D18 | `vaults`, `credentials`, `workspaces`, `environments` and `organizations` each have a column `version bigint not null default 1`, raised by one on every update by `stamp_change()` (D11). The controller's check per organization is the row count plus the sum of `version` across those tables, so it holds whatever the commit order. | User, 2026-11-23 ("yes" to a version column over the feed's log position) |

## Environments

| # | Decision | Source |
|---|---|---|
| D12 | Environments are one plain table `environments`, one row per environment, with a real `DELETE`. The `environment_revisions` table, the `environments` view, its `instead of` triggers and `revisions_append_only()` are dropped. | User, 2026-11-23 ("no more revisions. a proper table now. with dlete") |
| D13 | The migration copies each live revision into the new table before dropping `environment_revisions`; past revisions are not kept. | User, 2026-11-23 ("no more revisions"); memory `feedback-no-truncate` |
| D14 | `environments` and `organizations` become tracked tables: writer columns, `replica identity full`, the `history_stamp` and `history_deleted` triggers, a `history.<table>` table, a branch in `history.changes`, and an entry in Debezium's include list (`docker/debezium/application.properties`). The tracked tables are 20, and `docs/history.md` lists them. | User, 2026-11-23 (the brief); resource-history D2 |
| D15 | No route, response field or store method changes: every reader already reads the live view. | Plan audit, 2026-11-23 (dependencies A1) |

## Delivery

| # | Decision | Source |
|---|---|---|
| D16 | This spec ships as its own PR; the code ships as one PR of ordered commits under 100 files. | Memory `feedback-agentd-fewer-prs` |

## Secret cleanup

| # | Decision | Source |
|---|---|---|
| D17 | A secret left in the secret store after its credential row is deleted is purged from the change feed: a consumer beside the history consumer purges the secret each `credentials` delete event names (workspace, vault, credential), reading in a consumer group of its own (Kafka `agentd.<topic>.secret-purge`, a Valkey group `agentd-secret-purge` on the stream) so it shares no records or position with the history recorder, and commits its feed position only after the purge succeeds. A purge of a secret already gone succeeds, so a replay is harmless. The purge right after the commit stays as the fast path; the feed consumer covers an organization, vault or credential delete alike. A Postgres-only agentd runs no feed, so it has only the fast path. An archive's or rotation's secret purge reaches the feed as an update and is not retried from it. | User, 2026-11-23 ("but we have the change event from kafka"; no pending-purge table) |

Out of scope: [requirements.md](requirements.md#out-of-scope).
