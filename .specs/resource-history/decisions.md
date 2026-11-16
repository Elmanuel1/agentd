# Resource history: decisions

agentd keeps every change to its resources, with who made it and when, and an operator can roll any row back. The
live tables stay plain and the wire stays identical to Anthropic Managed Agents: history is an after-effect a change
feed consumer builds into separate storage ([ADR 0001](adr/0001-history-from-the-change-feed.md)). Each row names
who decided and when.

## What is kept

| # | Decision | Source |
|---|---|---|
| D1 | History is an after-effect of the change feed: a feed consumer builds it from the Debezium change feed into separate history storage. An action still does its own work in its request; a delete deletes inline, secrets and blobs included. | User, 2026-11-15 ("its after effects that feeds from the events"); user, 2026-11-16 ("yes, plan it") |
| D2 | One history table per entity, `history.<table>`, with every column of the live table and the change's `lsn`, `tx_id`, `op` and `committed_at`. | User, 2026-11-16 ("One per entity") |
| D3 | Every update is kept, the agent loop's included. Volume follows load: about 1.7 GB a day at 100 concurrent sessions and about 170 GB a day at the 10,000-session peak. | User, 2026-11-16 ("Yes, that way") |
| D4 | The tracked tables are the 18 resource tables: `agents`, `deployments`, `deployment_runs`, `files`, `vaults`, `credentials`, `skills`, `skill_versions`, `memory_stores`, `memories`, `memory_versions`, `sessions`, `dreams`, `dream_runs`, `api_keys`, `workspaces`, `environment_keys`, `admin_keys`. | User, 2026-11-16 (plan approved) |
| D5 | Environments keep their in-table revisions (`environment_revisions`, V7 and V8) and are not tracked. | User, 2026-11-16 (plan approved) |
| D6 | Coordination tables (`work_items`, `loop_steps`, `mcp_connections`, `schedules`, `fire_claims`, `schedule_outbox`, `webhook_deliveries`) keep no history. | User, 2026-11-03; restated 2026-11-16 |
| D7 | The session event log (`session_events`) keeps no history: it is the session's history, and it stays off Postgres-only features. | User, 2026-11-16 (plan approved); memory `project-agentd-event-log` |
| D8 | Redacting a memory version removes its content from history too: the records stay, never the value. | User, 2026-11-03 (memory decision D2) |
| D9 | No API route and no wire change: live has no history endpoint. Reading, searching and rolling back history are SQL runbooks in `docs/history.md`. | User, 2026-11-03 (D3, restore is a SQL runbook); memory `feedback-agentd-match-anthropic` |

## How it is captured

| # | Decision | Source |
|---|---|---|
| D10 | Each tracked table gains `actor` and `request_id`, stamped by a `BEFORE INSERT OR UPDATE` trigger from the writing connection's `agentd.actor` and `agentd.request_id`, so every change event carries its writer. | User, 2026-11-16 (plan approved) |
| D11 | A delete announces itself with a transactional logical message, `agentd-history`, from a `BEFORE DELETE` trigger: the table, the deleted row, the actor and the request id, committed or rolled back with the delete. | User, 2026-11-16 (plan approved) |
| D12 | Each tracked table has `REPLICA IDENTITY FULL`, so an update that leaves a TOASTed column unchanged still reaches the feed whole. | User, 2026-11-16 (plan approved) |
| D13 | The history schema is the one list of tracked tables: agentd reads its tables and column types once at boot and starts one recorder per table. A migration that adds a column to a tracked table adds it to its history table in the same migration. | User, 2026-11-16 (plan approved) |

## Storage and search

| # | Decision | Source |
|---|---|---|
| D14 | History storage sits behind a port, `HistoryLog`. Postgres is the default adapter, not partitioned; a ClickHouse adapter, chosen by a setting when it lands, is a later PR for deployments near the peak. | User, 2026-11-16 ("Yes, that way") |
| D15 | Search across entities: each history table has an index on `(actor, committed_at)` and one on `request_id`, and the view `history.changes` unions the shared columns of every history table, so "what did actor X do" and "what did request Y change" are one statement each. | User, 2026-11-16 ("Add view and indexes") |
| D16 | The change feed's slot is the one way to lose history. agentd exports the slot's retained and safe WAL, a lost flag, the WAL written and the history apply delay; the alarms belong to the infrastructure, and `docs/history.md` names their thresholds, the WAL sizing rule and the re-baseline runbook. | User, 2026-11-11 ("We just export metrics"); user, 2026-11-16 (plan approved) |
| D17 | Resource history ships as one agentd PR under 100 files, the spec included, in seven commits. | User, 2026-11-16 ("too much prs. keep below 100 files") |

## Out of scope

- The blob and secret content of a deleted file, skill, memory or credential: the delete removes it in the request
  (D1); history keeps the record.
- Moving environments to the feed history; the actor by name (a later PR).
- The ClickHouse adapter and its setting (D14).
