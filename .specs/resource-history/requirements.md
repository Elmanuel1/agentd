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
