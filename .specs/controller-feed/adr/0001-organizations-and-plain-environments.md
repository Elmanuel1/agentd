# ADR 0001: Organizations get a table, tracked rows get a change time and a version, environments become a plain table

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
2. A version per row that goes up on every update, so the controller can check per organization, without the feed,
   that it holds the changes: an order-independent sum of a 64-bit hash of each row's (table, id, `version`),
   which a delete paired with an insert changes too. A change time
   cannot serve: `updated_at` is the writing transaction's start time, and a transaction that starts first can
   commit last, leaving the count and the newest `updated_at` as they were. `vaults`, `credentials` and
   `workspaces` have only `created_at` (A7).
3. Environments in the feed. Migration V8 made `environments` a view over an append-only `environment_revisions`
   table, and a publication cannot carry a view (P1).

## Industry practice

- A tenant is a row, and tenant-owned rows reference it with a foreign key that cascades on delete. (Postgres docs,
  "Constraints: Foreign Keys", `ON DELETE CASCADE`).
- An `updated_at` column kept by a `before update` trigger is the standard way to stamp every write, whatever
  statement makes it (Postgres `contrib/spi` `moddatetime`; Rails and Django set the same column on every save).
- A row version raised on every update is the standard way to tell one write of a row from the next (Rails
  `lock_version`, JPA `@Version`).
- Change data capture reads base tables, and a row's history is the stream of its change events, not revision rows
  in the live table (Debezium Postgres connector docs). agentd's feed carries 18 tables this way.

Departures:

- The workspace-scoped tables carry no `org_id` and get no foreign key; the organization delete removes their rows
  explicitly in its transaction (decisions D5). Adding `org_id` and a foreign key to about 20 tables, and requiring
  a `workspaces` row before a workspace key, is a larger change the controller does not need.
- `stamp_change()` uses the transaction's `now()` instead of `moddatetime`'s `current_timestamp` per row: the
  same value, without the `spi` extension.

## Decision

1. A table `organizations` (`org_id`, `created_at`, `updated_at`, `version`, `actor`, `request_id`). `agentd organization
   create [--id <org_id>]` is the one way to make one.
2. A foreign key from each org-scoped table to `organizations`, `on delete cascade`. Key creation for an unknown
   organization fails.
3. `agentd organization delete <org_id>` deletes the organization and everything it owns in one transaction:
   workspace-scoped rows explicitly, org-scoped rows by cascade, and a schedule delete in the outbox for each removed
   deployment not yet archived. Secrets, file blobs, skill bundles and memory contents go after the commit;
   an hourly sweep removes any secret or blob no row names, so an interrupted delete leaves none.
4. `version` and `updated_at` on `vaults`, `credentials`, `workspaces`, `environments` and `organizations`, kept by
   one trigger: `version` starts at 1 and goes up by one on every update; `updated_at` is for display and debugging.
5. `environments` is a plain table again, its live rows copied from `environment_revisions`, which is dropped with
   the view, its triggers and `revisions_append_only()`.
6. `environments` and `organizations` join the tracked tables and Debezium's include list.

## Consequences

- The feed carries every organization, environment, workspace, vault and credential change, each delete included.
- Past environment revisions are not kept; an environment's changes are its feed events, as every tracked table's
  are, and agentd keeps no history store (decisions D21).
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
- **Order by the feed's log position.** The check exists to catch what the feed missed, so it must not depend on
  the feed; the user chose a version column.
- **Set `updated_at` in Rust.** Ten insert and update statements, three of them raw SQL, would each have to set it, and a
  missed one breaks the controller's check silently.
