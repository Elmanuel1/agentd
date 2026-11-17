# Requirements: resource history

**Date:** 2026-11-16
**Format:** EARS (Easy Approach to Requirements Syntax). Each requirement uses one pattern:

| Pattern | Template |
|---|---|
| Ubiquitous | The `<system>` shall `<response>`. |
| Event-driven | When `<trigger>`, the `<system>` shall `<response>`. |
| Unwanted behaviour | If `<condition>`, then the `<system>` shall `<response>`. |

**Systems:** *the database* (agentd's Postgres schema: its triggers and history tables), *the change feed*
(Debezium Server reading Postgres's WAL, and agentd's delivery of it to handlers), *the history consumer*
(agentd's change feed handlers that write history), *the history store* (the `HistoryLog` port and its adapter),
*agentd* (the one binary).

**Terms:**
- **Tracked table:** one of the 18 resource tables of decision D4.
- **History table:** `history.<table>`, a tracked table's history.
- **Actor, request id:** the writing connection's `agentd.actor` and `agentd.request_id` settings.

**Build order:** spec → the feed's positions and messages (HI-4) → capture, V16 (HI-1 to HI-3) → storage, V17 and the
port (HI-7, HI-14, HI-15) → the consumer (HI-5 to HI-7, HI-12, HI-13, HI-16) → redaction (HI-8) → metrics and runbooks
(HI-9 to HI-11), one PR (D17).

Source decisions: [decisions.md](decisions.md). Evidence: [dependencies.md](dependencies.md). The decision record:
[ADR 0001](adr/0001-history-from-the-change-feed.md).

## Capture

| ID | Requirement |
|---|---|
| HI-1 | When a row of a tracked table is inserted or updated, the database shall set its `actor` and `request_id` from the writing connection's settings. |
| HI-2 | When a row of a tracked table is deleted, the database shall emit, in the deleting transaction, a logical message `agentd-history` carrying the table, the deleted row, the actor and the request id. |
| HI-3 | Each tracked table shall have `REPLICA IDENTITY FULL`. |
| HI-4 | The change feed shall hand each handler a change's LSN, transaction id and commit time, and logical messages of the `agentd-history` prefix. |

## Recording

| ID | Requirement |
|---|---|
| HI-5 | When the feed delivers an insert or update of a tracked table, the history consumer shall record the whole row as committed, with its LSN, commit time, actor and request id. |
| HI-6 | When the feed delivers an `agentd-history` message, the history consumer shall record a delete of that row with the message's actor and request id. |
| HI-7 | The history consumer shall record each change once, however often the feed delivers it. |
| HI-8 | When a memory version is redacted, the history consumer shall remove that content from the history of the version and of every memory head that held it. |
| HI-13 | The history consumer shall record every update of a tracked table, the agent loop's included. |
| HI-16 | If Debezium Server, the feed's broker, an agentd replica or Postgres stops, crashes, hangs or loses its connections, then once it is back the history consumer shall record every change committed before and during the fault, once and in commit order, while the feed keeps its records (the slot is not lost, and no record expires or is lost by its broker before it is read). |

## Storage and search

| ID | Requirement |
|---|---|
| HI-14 | Each tracked table shall have a history table with the same columns and types, plus `lsn`, `tx_id`, `op` and `committed_at`. |
| HI-15 | The history store shall answer the changes of one actor in a time range, and of one request id, across every tracked table, in one statement that reads each history table by index. |

## Operations

| ID | Requirement |
|---|---|
| HI-9 | agentd shall export the slot's retained WAL, safe WAL, lost flag, the WAL written, and the history apply delay. |
| HI-10 | An operator shall be able to roll back any recorded row with one documented statement, recorded as a change by `operator`. |
| HI-11 | The docs shall give the WAL sizing rule, the alarm thresholds and the re-baseline runbook after a lost slot. |
| HI-12 | No response body, status, header or route shall change. |

## Rollout

- agentd is not deployed (runtime-parity D45): the PR merges on green CI and a local run against Postgres 16, Kafka
  and Debezium Server.
- A deployment runs V16 and V17 with Flyway (V16 sets a 5 s `lock_timeout`; rerun it when a busy table refuses the
  locks), makes the Kafka topics `agentd.public.<table>` for the 18 tracked tables and `agentd.message`, adds the
  tables to the publication, then restarts Debezium Server with the longer `table.include.list`.
- A database that already has rows runs the baseline of `docs/history.md` once, so every live row has a history row.
- `max_slot_wal_keep_size` is sized as `docs/history.md` says before the tracked tables join the feed.

## Out of scope

- The blob and secret content of a deleted file, skill, memory or credential: the delete removes it in the request
  (D1); history keeps the record.
- Moving environments to the feed history (D5); the actor by name.
- The ClickHouse adapter and its setting (D14).
- An API route for history (D9).
