# ADR 0001: Organizations get a table, tracked rows get a change time, environments become a plain table

Status: Proposed
Date: 2026-11-23
Deciders: the user (Elmanuel1)
Scope: agentd's Postgres schema, the `agentd organization` CLI, the key commands, and Debezium's include list; the
wire does not change

## Context

The agentd-egress-proxy-controller builds its Kubernetes objects from agentd's Debezium change feed and
reconciles per organization. It needs three things the schema lacks:

1. Organization rows and their deletes in the feed. agentd keeps no organization record; an org id is whatever
   `--org` a key command was given (dependencies A3).
2. A change time per row, so a per-organization check of row count plus latest `updated_at` can tell whether the
   controller has missed a change. `vaults`, `credentials` and `workspaces` have only `created_at` (A7).
3. Environments in the feed. Migration V8 made `environments` a view over an append-only `environment_revisions`
   table, and a publication cannot carry a view (P1).

## Industry practice

- A tenant is a row, and tenant-owned rows reference it with a foreign key that cascades on delete. (Postgres docs,
  "Constraints: Foreign Keys", `ON DELETE CASCADE`).
- An `updated_at` column kept by a `before update` trigger is the standard way to stamp every write, whatever
  statement makes it (Postgres `contrib/spi` `moddatetime`; Rails and Django set the same column on every save).
- Change data capture reads base tables; history lives in the CDC consumer's store, not in revision rows in the
  live table (Debezium Postgres connector docs). agentd already does this for 18 tables (resource-history ADR
  0001).

Departures:

- The workspace-scoped tables carry no `org_id` and get no foreign key; the organization delete removes their rows
  explicitly in its transaction (decisions D5). Adding `org_id` and a foreign key to about 20 tables, and requiring
  a `workspaces` row before a workspace key, is a larger change the controller does not need.
- `stamp_updated_at()` uses the transaction's `now()` instead of `moddatetime`'s `current_timestamp` per row: the
  same value, without the `spi` extension.

## Decision

1. A table `organizations` (`org_id`, `created_at`, `updated_at`, `actor`, `request_id`). `agentd organization
   create [--id <org_id>]` is the one way to make one.
2. A foreign key from each org-scoped table to `organizations`, `on delete cascade`. Key creation for an unknown
   organization fails.
3. `agentd organization delete <org_id>` deletes the organization and everything it owns in one transaction:
   workspace-scoped rows explicitly, org-scoped rows by cascade. Secrets and file blobs go after the commit.
4. `updated_at` on `vaults`, `credentials`, `workspaces`, `environments` and `organizations`, set by one trigger.
5. `environments` is a plain table again, its live rows copied from `environment_revisions`, which is dropped with
   the view, its triggers and `revisions_append_only()`.
6. `environments` and `organizations` join the tracked tables and Debezium's include list.

## Consequences

- The feed carries every organization, environment, workspace, vault and credential change, each delete included.
- Past environment revisions are gone; environment history comes from the feed like every other tracked table, so
  a Postgres-only agentd keeps none (resource-history D5).
- Every script that makes keys first runs `agentd organization create`: CI, docker-compose, perf, conformance and
  the configuration docs.
- An organization delete is one long transaction for a large organization; it is an operator command, not a
  request path.

## Alternatives rejected

- **Create the organization row on the first admin key.** Workspace keys and environment keys also name an org,
  and docker-compose makes only workspace keys, so every key command would need the same upsert; the user chose
  an explicit command.
- **Keep revisions and publish `environment_revisions`.** The controller would rebuild the live row from revision
  rows; the user chose "no more revisions".
- **Set `updated_at` in Rust.** Twelve write statements, three of them raw SQL, would each have to set it, and a
  missed one breaks the controller's check silently.
