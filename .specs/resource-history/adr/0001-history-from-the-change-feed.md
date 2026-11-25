# ADR 0001: Resource history is built from the change feed into one history table per entity

Status: Superseded by controller-feed decision D21 (`.specs/controller-feed/decisions.md`): agentd keeps no history store
Date: 2026-11-16
Deciders: the user (Elmanuel1)
Scope: agentd's resource tables, its change feed and a new history store; the wire does not change

## Context

agentd must keep every change to its resources (agents, sessions, vaults, keys and the rest), with who made it and
when, so an operator can trace and roll back any row. The wire must stay identical to Anthropic Managed Agents,
which has no history endpoint, so history cannot live in the API.

agentd already has a change feed: Debezium Server reads Postgres's WAL into Kafka or Valkey Streams, and agentd's
handlers react to it, at least once and idempotently. An action does its own work in its request; only after-effects
(the schedule outbox, webhook deliveries) feed from it.

A change event carries the row before and after, but not who made it. agentd's writes already set the writer on
the connection (`agentd.actor`, `agentd.request_id`), but the resource tables do not record it.

## Decision

1. History is an after-effect: a change feed consumer builds it into separate storage. The live tables stay plain;
   their queries and the wire do not change.
2. The writer rides on the row. Each tracked table gains `actor` and `request_id`, stamped by a `BEFORE INSERT OR
   UPDATE` trigger from the connection's settings, so every insert and update event is complete on its own.
3. A delete announces itself. A `BEFORE DELETE` trigger emits a transactional logical message, `agentd-history`,
   with the table, the deleted row and its writer; it commits or rolls back with the delete. The feed's own delete
   events are not used, since they carry the row's last writer, not the deleter.
4. Each tracked table has `REPLICA IDENTITY FULL`, so an update that leaves a large column unchanged still arrives
   whole.
5. One history table per entity, `history.<table>`, with the live table's columns and the change's LSN,
   transaction id, operation and commit time. Its key is the live key plus the LSN, so a redelivered change adds
   nothing. Indexes on `(actor, committed_at)` and `request_id`, and the view `history.changes` over every history
   table, answer "what did this actor do" and "what did this request change" in one statement.
6. Storage sits behind the `HistoryLog` port. Postgres is the default adapter; a ClickHouse adapter is a later PR.
7. The slot is the one way to lose history, so agentd exports its retained and safe WAL and a lost flag, and the
   docs give the sizing rule and the re-baseline runbook.

## Alternatives considered

| Alternative | Why not |
|---|---|
| In-table revisions per entity (each update inserts a new row, as environments do) | Every query and write of every entity changes; the live tables stop being plain. |
| Debezium's audit pattern: a per-transaction context table joined to the changes by `txId` | Needs a stateful join across topics, which agentd's per-table handlers do not have. |
| Every insert, update and delete emitted as a logical message by a trigger | History becomes a copy the trigger writes rather than the change Postgres decodes, and every change rides on logical messages, whose Debezium handling has open issues (DBZ-8185). |
| Debezium's reselect post-processor for unchanged TOAST columns | It reads the row as it is now, not as it was at the change. |
| One shared history table with the row as `jsonb` (PaperTrail, `supa_audit`) | The user chose one table per entity; typed columns keep a rollback one typed statement. |

## Consequences

- 18 tables gain two columns, two triggers and `REPLICA IDENTITY FULL`; an update writes the old row to the WAL
  too, about twice the WAL of a session update.
- The slot keeps all WAL while the feed is down, so `max_slot_wal_keep_size` is sized from the peak WAL rate and
  the longest outage to ride out; a lost slot leaves a gap that a re-baseline closes from then on.
- History in Postgres grows about 170 GB a day at the peak; large deployments move to the ClickHouse adapter.
- A migration that adds a column to a tracked table adds it to its history table too; a test fails otherwise.
- Reading, searching and rolling back history are SQL runbooks, with no API route.
