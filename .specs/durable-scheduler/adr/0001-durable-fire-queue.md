# ADR 0001: Every due occurrence is a row in a durable fire queue before it runs

Status: Proposed
Date: 2026-11-16
Deciders: the user (Elmanuel1)
Scope: agentd's scheduler (the tick, the fire inbox, the change feed's fire consumers) and its optional Temporal sink

## Context

agentd fires scheduled deployments. Terms:

- An **occurrence** is one listed time of a schedule (`scheduled_at`), as `upcoming_runs_at` shows it.
- The **due time** (`due_at`) is the occurrence plus live's jitter (D5), the earliest moment its fire may run.
- A **business failure** is a fire whose session cannot be created; its run is recorded with an `error`. An **infra
  failure** is any other failure: the database is unavailable, the handler times out, or the replica stops mid-fire.

On `main`, each replica's tick claims up to 100 due `schedules` rows every 10 s, moves each to its next time,
commits, and only then fires them one after another. A crash or a failed fire after that commit loses the
occurrence, since the schedule has already moved on. A fire more than 60 s late is skipped with no log line, and a
burst of more than about 600 schedules a minute per replica is served late and then skipped.

## Decision

1. The tick writes each due occurrence into `fire_claims`, with its spec and due time, in the same transaction that
   moves its schedule on. It claims batches of 100 until a batch comes back short.
2. Every replica runs the due, undone rows of `fire_claims`, claimed with `FOR UPDATE SKIP LOCKED` under a 2-minute
   lease, up to 16 at a time, each with a 60 s timeout. A claim's `attempts` is its token: a worker finishes or
   retries its fire only while the row still holds that token, so a worker whose lease was taken over writes nothing.
3. A fire that records a run, with a session or with an error, is done (`outcome` `ran`). An infra failure is run
   again after a backoff from 2 s doubling to 5 minutes, with jitter.
4. A fire still not run when its schedule's following occurrence is due is done with `outcome` `skipped`, logged at
   `WARN` (D1). After downtime the tick writes only each schedule's latest missed occurrence.
5. Fires an external engine delivers on `agentd.fires` are written into `fire_claims` the same way and run by the
   same workers.
6. Temporal is an optional sink behind `SchedulerPort` (D2). It holds the timers; each Temporal action writes its
   occurrence into `fire_claims`, and agentd's workers run it.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Keep firing after the commit and retry failed fires inside the tick | A crash between the commit and the fire still loses the occurrence; the retry state would live in a replica's memory. |
| A new `fires` table beside `fire_claims` | `fire_claims` already holds one row per occurrence with the dedupe key; two tables would split one aggregate. |
| Temporal runs the fire itself in an activity | Two run paths with different dedupe and retry rules; agentd's queue keeps one path for every sink. |
| One leader replica inserts the occurrences (Oban, River) | Row locks and the occurrence's unique key already stop a double insert; a leader adds an election for no gain. |

## Consequences

- An occurrence can no longer be lost once its schedule has moved on, and a top-of-hour burst is spread by the
  jitter and run concurrently.
- `fire_claims` gains `spec`, `due_at` and `outcome` (V16) and two partial indexes (V17); done rows are deleted 30
  days after their occurrence (D12).
- The 15-minute lease and 3-attempt limit of runtime-parity K-8 are replaced by the 2-minute lease and the backoff
  bounded by the following occurrence.
- With Temporal on, two durable layers hold a fire: Temporal's timer and agentd's queue.
