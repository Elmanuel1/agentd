# Requirements: durable scheduler

**Date:** 2026-11-16
**Format:** EARS (Easy Approach to Requirements Syntax). Each requirement uses one pattern:

| Pattern | Template |
|---|---|
| Ubiquitous | The `<system>` shall `<response>`. |
| Event-driven | When `<trigger>`, the `<system>` shall `<response>`. |
| State-driven | While `<state>`, the `<system>` shall `<response>`. |
| Unwanted behaviour | If `<condition>`, then the `<system>` shall `<response>`. |
| Optional feature | Where `<feature>`, the `<system>` shall `<response>`. |

**Systems:** *agentd* (the one binary, in any role), *the API* (agentd serving requests), *the scheduler* (agentd's
tick, which turns due schedules into occurrences, and its fire workers, which run them), *the change feed* (Debezium
Server reading Postgres's WAL, and agentd's consumers of it).

**Terms:**
- **Live:** Anthropic's hosted Managed Agents API, the behaviour agentd matches.
- **Occurrence:** one listed time of a schedule (`scheduled_at`), as `upcoming_runs_at` shows it.
- **Following occurrence:** the schedule's next listed time after an occurrence.
- **Due time:** the occurrence plus its jitter (DS-3), the earliest moment its fire may first run. It is the same
  for every delivery of the occurrence, so it can always be worked out again from the spec and `scheduled_at`.
- **`due_at`:** the column holding the earliest moment a fire may run next: its due time until a retry moves it out.
- **Claim token:** the `attempts` value a worker's claim set; a worker writes its fire's row only while `attempts`
  still holds its token.
- **Fire queue:** the `fire_claims` table, one row per occurrence until it is done.
- **Business failure:** a fire whose session cannot be created; its run is recorded with an `error`.
- **Infra failure:** any other failed fire: the database is unavailable, the handler times out, or the replica stops
  mid-fire.

**Build order:** deployment writes and fired creates (DS-14, DS-20) → fire queue columns → fire queue and workers (DS-3 to DS-7, DS-18,
DS-19) →
tick and feed on the queue (DS-1, DS-2, DS-8 to DS-10, DS-12) → schedule changes (DS-11) → Kafka (DS-13) → metrics
and retention (DS-15, DS-16) → Temporal (DS-17).

Source decisions: [decisions.md](decisions.md). Evidence: [dependencies.md](dependencies.md). The decision record:
[ADR 0001](adr/0001-durable-fire-queue.md). These rows supersede runtime-parity K-6, K-8 and K-9 and narrow K-4
([runtime-parity requirements](../runtime-parity/requirements.md), section 6).

## 1. Occurrences

| ID | Requirement |
|---|---|
| DS-1 | When the scheduler moves a due schedule to its next time, the scheduler shall write that occurrence to `fire_claims`, with the schedule's spec and the occurrence's due time, in the same transaction. |
| DS-2 | The scheduler shall claim due schedules in batches of 100, each batch in its own transaction, until a batch finds fewer than 100. |
| DS-3 | The scheduler shall set an occurrence's due time to its listed time plus a delay of up to 15 percent of the time to its following occurrence, at least 5 s and at most 9 minutes, the same for every delivery of that occurrence (D5). |
| DS-10 | When a schedule is unpaused, the scheduler shall move it to its next occurrence after the unpause (D6, D14). |
| DS-12 | If a due schedule's spec cannot be read, then the scheduler shall pause it and log its target at `ERROR`. |

## 2. Running fires

| ID | Requirement |
|---|---|
| DS-4 | The scheduler shall run, on every replica, the occurrences in `fire_claims` that are due and not done, claimed with `FOR UPDATE SKIP LOCKED` under a 2-minute lease, up to 16 at a time per replica. |
| DS-19 | If a worker finishes or retries a fire whose claim token another worker has since replaced, then the scheduler shall leave the row unchanged and log the lost lease at `WARN`. |
| DS-5 | If a fire fails with an infra failure, then the scheduler shall run it again after a backoff from 2 s doubling to 5 minutes, with jitter, until its following occurrence is due. |
| DS-6 | If a fire runs longer than 60 s, then the scheduler shall count it as an infra failure (DS-5). |
| DS-7 | When a fire records a run, with a session or with an error, the scheduler shall mark it done with outcome `ran` and not run it again (D7). |
| DS-8 | If an occurrence is not run by the time its following occurrence is due, then the scheduler shall mark it done with outcome `skipped` and log it at `WARN`, with no backfill; after downtime, the scheduler shall write only each schedule's latest missed occurrence to `fire_claims` (D1). |
| DS-18 | When the scheduler marks a fire done, the scheduler shall record its outcome in `fire_claims.outcome`: `ran`, `skipped`, or `dropped` when no handler owns its kind or its spec cannot be read (D3). The column carries no `CHECK`: `Outcome::as_str` is the one list of its values. |
| DS-9 | When a fire arrives on the change feed's `agentd.fires`, the scheduler shall write it to `fire_claims` as DS-1 and DS-3 do, and acknowledge the delivery once that write commits. |
| DS-20 | If two creates of a session with the same requested id overlap, then the API shall return the session the first insert made to the create whose insert fails on that id, queue the session's initial events once, from whichever create appends them first, and leave the session's work item as the first queue write made it, matching that session (D15). |
| DS-14 | When the API writes a deployment it read earlier (a fire's bookkeeping, a pause, an unpause or an archive), the API shall write only if the deployment has not changed since that read, and otherwise read it again and redo the change. |

## 3. Schedule changes and the change feed

| ID | Requirement |
|---|---|
| DS-11 | If the scheduler sink refuses a schedule change as invalid, then the change feed shall log it at `ERROR` and pass over it; a change the sink cannot apply because it is unavailable still holds the stream. |
| DS-13 | While the Kafka change feed handles or retries a record, the change feed shall keep polling, and pause and move back any partition that delivers a record meanwhile, so that the consumer stays in the group, no second replica reads the partition, and no record behind it is handled first. |

## 4. Operations

| ID | Requirement |
|---|---|
| DS-15 | The scheduler shall delete, every hour, the `fire_claims` rows whose occurrence is more than 30 days old and that are done (D12). |
| DS-16 | The scheduler shall export the fires by outcome (`enqueued`, `ran`, `retried`, `skipped`, `dropped`), the fire lag (a run's start minus its occurrence's due time, worked out from the spec, not from `due_at`) and the count of due fires not done (D10). |
| DS-17 | Where `AGENTD_SCHEDULER` is `temporal`, agentd shall apply schedule changes to Temporal Schedules, not run its built-in tick, and write each Temporal action's occurrence to `fire_claims` as DS-9 does, with a catch-up window of ten years, longer than any gap between a cron's occurrences (D2). |

## Industry practice

| Pattern | Source | Here |
|---|---|---|
| The timer and the job it starts written in one transaction; a worker pool claims jobs with `SKIP LOCKED`, retries with backoff and dedupes by a unique key | Oban Cron, River periodic and unique jobs, pg-boss (I5) | Followed: the tick writes the occurrence into `fire_claims` in its own transaction; workers claim it. Departure: every replica ticks instead of one leader, since row locks and the occurrence's key already stop a double insert. |
| Retry with exponential backoff up to a maximum age, then give up with a record of why | EventBridge Scheduler retry policy and dead-letter queue (I3) | Followed for infra failures, bounded by the following occurrence; `outcome` records why a fire finished. Departure: business failures are not retried, because live does not retry them (L3). |
| After downtime, fire the latest missed occurrence once and resume | Quartz cron misfire policy (I4); Kubernetes CronJob; Temporal with overlap `Skip` (I1) | Followed (D1). |
| Jitter on fires that share a listed time | Live (L1); Temporal spec jitter (I1) | Followed with live's formula (D5). |
| A durable engine owns the timers | Temporal Schedules | Followed for the Temporal sink (D2). Departure: Temporal delivers the occurrence and agentd's queue runs it, so every sink has one run path. |
| Pause a partition and keep polling while a record is handled or retried | librdkafka (I6) | Followed (DS-13): only a partition that delivers a record meanwhile is paused. |

## Plan: one PR

The code lands as one PR after this spec, built as the ordered commits below, as the resource history PR was. Each
commit builds and passes the suite on its own; a split the 200-line rule forces happens inside the commit that needs
it. The PR touches 63 files, under CodeRabbit's 100-file review limit.

| Commit | What | Requirements |
|---|---|---|
| 1 | A deployment write never overwrites a newer one; an overlapping fired create keeps the first session | DS-14, DS-20 |
| 2 | Migrations V22 and V23: the fire queue columns and indexes | none (schema for DS-1, DS-15, DS-18) |
| 3 | The fire queue and its workers, not yet called | DS-3 to DS-7, DS-18, DS-19 |
| 4 | The tick and the feed on the queue | DS-1, DS-2, DS-8, DS-9, DS-10, DS-12 |
| 5 | An invalid schedule change is passed over | DS-11 |
| 6 | A Kafka consumer keeps its group while it retries | DS-13 |
| 7 | Metrics, retention and docs | DS-15, DS-16 |
| 8 | Temporal as a scheduler sink | DS-17 |

### Commit 1: a deployment write never overwrites a newer one

- `store-postgres/src/entities/deployments.rs`: `replace` takes the `updated_at` it read. Statement:
  `UPDATE deployments SET name, status, archived_at, doc WHERE workspace_id = $1 AND deployment_id = $2 AND
  doc->>'updated_at' = $3`, then the outbox `INSERT` and `DELETE` only when one row changed. Zero rows for an existing
  deployment returns `Stale`. The file does not grow.
- `deployments/src/service/rewrite.rs` (new): read, apply the change, guarded write, redo on `Stale` up to 5 times.
  `service/firing.rs` and the pause, unpause and archive paths in `service.rs` call it; `service.rs` does not grow.
- Statements for a fire's bookkeeping: 1 `SELECT` of the deployment, 1 guarded `UPDATE`, and the outbox's 2 when a
  schedule change applies; a redo repeats the first two.
- `store-postgres/src/entities/sessions.rs`: a session insert that fails on its primary key returns
  `StoreError::Conflict`. `sessions/src/service/lifecycle.rs`: on `Conflict` for a create with a requested
  id, `create_fired` returns the session through `made` and does not call `end_work`, so the losing create never stops
  the winner's work item (DS-20). Both creates queue their initial events through the event log's `append_if_empty`,
  which appends under the session's append lock only while its log is empty, so the first append lands and the
  other is a no-op. Its queue write is the existing upsert, which leaves a work item that is not
  requeuable as it is. Whichever create queues first and whichever inserts first, the work item matches the winning
  session: both creates fire one occurrence of one deployment, and a deployment's agent and environment are fixed at
  its create (no route updates them). The work item keeps the queue the first queue write chose from the environment
  it read; an environment update between the two creates acts as one made after the create, as it does for any
  session, since an environment update moves no queued work item. The file stays under 200 lines.
- Tests: `pg_deployments` "a pause between a fire's read and its write survives the fire"; unit "rewrite redoes on a
  stale write and gives up after 5"; `sessions` "two overlapping fired creates of one occurrence make one session and
  keep its work item queued", run for both orders (one create queues first while the other inserts first), asserting
  one session, one queued work item whose environment and session are the winning session's and whose queue is the one
  the first queue write chose (also when the environment's config type changes between the two creates), and one
  deployment run
  whose `session_id` is that session and whose `error` is null; `sessions` "the initial events of two overlapping
  creates are queued once by whichever appends first", for both append orders; `pg_event_log` "concurrent appends into
  an empty log land once and none into a log with events".

### Commit 2: migrations V22 and V23

- The versions follow `main`'s V20 and V21, as Flyway refuses a lower version after a higher one.
- `V22__fire_queue.sql`: `alter table fire_claims add column spec jsonb, add column due_at timestamptz, add column
  outcome text;`. The columns are nullable and nothing reads them yet; `outcome` is null until a fire is done. agentd
  is not in production, so no claim `take` wrote needs a backfill.
- `V23__fire_queue_indexes.sql`, in its own file as V12: `create index concurrently fire_claims_due_idx on
  fire_claims (due_at) where not done;` and `create index concurrently fire_claims_done_idx on fire_claims
  (scheduled_at) where done;`.
- Tests: the migration suite; `pg_fires` passes unchanged.

### Commit 3: the fire queue and its workers, unused

The PR's build opens with probes B33 and B34.

- `shared/src/ports/fire/claims.rs`: `Fire { spec, scheduled_at, due_at }`; `FireClaims` gains `enqueue`,
  `claim_due(limit)` and `retry(&Fire, at)`, and `finish` takes an `Outcome`. `take` stays until commit 4.
- `shared/src/ports/fire/worker.rs` (new): `FireWorker::fill` claims a due fire for each of its 16 slots that is
  free, every second and whenever a fire finishes, runs each through `FireRegistry` with a 60 s timeout, then
  finishes or retries it within 10 s, and reports each outcome and lag to a callback
  the api crate passes in.
- `shared/src/ports/fire/jitter.rs` (new): the due time of DS-3, from `CompiledSchedule::next_fires`;
  `shared/src/schedule.rs` is not changed.
- `store-postgres/src/entities/fire_claims.rs` becomes `fire_claims/{claim.rs, queue.rs}`:
  - `enqueue_in(c, &[Fire])`, used inside the tick's transaction, and `enqueue`: 1 `INSERT INTO fire_claims
    (workspace_id, target, scheduled_at, spec, due_at, claimed_at, attempts) SELECT … FROM unnest(…)` with
    `claimed_at` `'-infinity'` and `attempts` 0, `ON CONFLICT DO NOTHING`.
  - `claim_due`, one transaction: 1 `SELECT workspace_id, target, scheduled_at, spec, due_at FROM fire_claims WHERE
    NOT done AND due_at <= now() AND claimed_at < now() - interval '120 seconds' ORDER BY due_at LIMIT $1 FOR UPDATE
    SKIP LOCKED`; 1 `UPDATE fire_claims SET claimed_at = now(), attempts = attempts + 1 FROM unnest(…) … RETURNING
    attempts` on the keys. Each claimed fire carries its `attempts` as its claim token.
  - `finish`: 1 `UPDATE fire_claims SET done = true, outcome = $5` on the key `AND attempts = $4`. `retry`: 1
    `UPDATE fire_claims SET due_at = $5, claimed_at = '-infinity'` on the key `AND attempts = $4 AND NOT done`. Zero
    rows means another worker took the lease: the row stays as it is and the lost lease is logged (DS-19).
  - The lag of DS-16 is measured from the due time worked out again from the spec (DS-3), so a retry that moves
    `due_at` does not shorten it.
- Tests: `pg_fire_queue` (new) "an enqueued occurrence is claimed once across two workers", "a claim whose worker
  stopped is taken again after its lease", "a retried fire waits for its backoff", "enqueuing an occurrence twice
  keeps the first due time", "a done fire records ran, skipped or dropped", "a worker whose lease was taken over
  cannot finish or retry the fire"; unit tests for the jitter (bounds for
  minute, hourly and daily crons; the same occurrence gets the same delay) and for the worker (ran, business failure,
  timeout, infra failure).

### Commit 4: the tick and the feed on the queue

- `schedules/tick.rs`, per batch in one transaction: the existing `SELECT … FOR UPDATE SKIP LOCKED`, `enqueue_in`,
  the existing `UPDATE … FROM unnest`, 1 `UPDATE schedules SET paused = true WHERE target = ANY($1)` for unreadable
  rows, and the existing `DELETE`; repeated while a batch is full. Firing after the commit and the 60 s skip are
  removed, and `PgScheduler` no longer holds the registry. The file does not grow.
- `schedules.rs`: unpause reads the schedule and the database's `now()`, then writes 1 `UPDATE schedules SET paused =
  false, next_fire_at = $2 WHERE target = $1 AND paused AND spec = $3 AND $2 > clock_timestamp()`, the next
  occurrence after that `now()`; a write that matches no row reads again, up to 3 times.
- `shared/src/ports/fire/worker.rs`: the DS-8 window is checked before a run.
- `shared/src/ports/fire/delivery.rs`: `deliver` enqueues instead of running and answers `true` once the insert
  commits; `changes-kafka` and `signals-valkey/src/fires.rs` call it unchanged.
- `api/src/backends/postgres.rs`: `FireSources::start` spawns the tick and the worker (every 1 s).
- `fire_claims/claim.rs`: `take` and its 15-minute and 3-attempt rules are removed, with their `pg_fires` tests.
- Tests: `pg_schedules` "a due occurrence fires once across two schedulers" over the queue; "250 due schedules all
  fire in one tick", with 150 older due rows first; "a fire that fails is run again, then done"; "a crash after the
  tick commits loses nothing"; "after downtime only the latest missed occurrence fires"; "a fire not run before its
  following occurrence is skipped"; "an unpaused schedule does not fire the time it missed"; "an unreadable schedule
  is paused and does not hold the batch"; "unpausing an active schedule keeps its due occurrence".

### Commit 5: an invalid schedule change is passed over

- `schedules/outbox.rs`: `SchedulerError::InvalidSpec` is logged at `ERROR` with its target and returns `Ok`;
  `Unavailable` still returns an error. If probe B34 shows live refuses a never-firing cron, deployment create refuses
  it too, in `deployments/src/service.rs`; `types.rs` does not grow.
- Test: `pg_schedules` "an invalid change is passed over and the next one applies".

### Commit 6: a Kafka consumer keeps its group while it retries

- `changes-kafka/src/retry.rs`: every attempt at a record, and every backoff between attempts, runs while the
  consumer keeps polling. A record polled meanwhile pauses its partition, which is moved back to that record (or,
  for the record's own partition, to just past the record being retried) and read again afterwards; the assignment
  resumes only when something was held back. A revoked partition ends the attempt, checked every second and after a
  success, and nothing is committed, so its next owner reads the record. `lib.rs` assigns partitions
  `cooperative-sticky`: a join moves only the partitions balance needs, so a one-partition topic stays with its
  holder.
- Tests: `changes-kafka/tests/kafka.rs` "a record retried past max poll interval keeps one reader", with
  `max.poll.interval.ms` at 10 s and a record failing for 20 s: no second consumer is assigned the partition and the
  record applies once; "a retry attempt longer than max poll interval keeps one reader"; "a first attempt longer than
  max poll interval keeps one reader"; "a record whose partition is revoked mid attempt is applied once by its next
  owner".

### Commit 7: metrics, retention and docs

- `api/src/telemetry/scheduler.rs` (new): counter `agentd.scheduler.fires` by `outcome`, histogram
  `agentd.scheduler.fire.lag` (s), gauge `agentd.scheduler.fires.due` from 1 `SELECT count(*) FROM fire_claims WHERE
  NOT done AND due_at <= now()` every 30 s per replica.
- `fire_claims/sweep.rs` (new): hourly, 1 `DELETE FROM fire_claims WHERE done AND scheduled_at < now() - interval
  '30 days'` on the `done` index.
- Docs: `configuration.md` "Scheduled work" describes the queue, the jitter, retries and the window;
  `observability.md` names each signal and its threshold for any backend (due fires above 0 for 5 minutes, lag p99
  over 60 s, any `skipped` or `dropped`); `operations.md` says every role runs the feed handlers; the
  `scheduler.rs` module doc and the README's backend line describe the queue.
- Tests: telemetry unit tests; `pg_fire_queue` "done rows older than 30 days are swept, newer and undone rows stay".

### Commit 8: Temporal as a scheduler sink

- `crates/adapters/scheduler-temporal` (new): `TemporalScheduler: SchedulerPort` over Temporal Schedules. `upsert`
  creates or updates a schedule with the cron and timezone, jitter 0, overlap `AllowAll` and a catch-up window of ten
  years, longer than any gap between a cron's occurrences (Temporal's action buffer still bounds how many it delivers
  after an outage, and the fire queue skips one whose following occurrence is due); `pause`, `unpause`, `delete` and `describe` map one to one. A Temporal worker in agentd
  runs one activity that writes the action's occurrence (`ScheduledStartTime` as `scheduled_at`) through
  `FireClaims::enqueue`, retried by Temporal until it commits.
- `api/src/config` and `backends.rs`: `AGENTD_SCHEDULER` (`postgres` by default, or `temporal`),
  `AGENTD_TEMPORAL_ADDRESS`, `AGENTD_TEMPORAL_NAMESPACE`, `AGENTD_TEMPORAL_TASK_QUEUE`; with `temporal`, the built-in
  tick does not run.
- Dependencies: `temporalio-client` and `temporalio-sdk` (I2).
- Tests: a Temporal dev server in CI (`temporal server start-dev`); "a schedule on the Temporal sink fires through the
  queue once", which drives the sink directly; "pause and unpause reach the Temporal schedule", which also reads back
  the ten-year catch-up window.

## Sizing

A top-of-hour burst of 10,000 hourly schedules (ten organizations at live's 1,000-deployment limit) is 100 batches of
4 statements, about 2 s of tick time. The jitter spreads the fires over up to 9 minutes, about 19 a second across
the cluster. One replica running 16 fires of about 50 ms each at once runs about 300 a second.

## Out of scope

- The Valkey change feed's ordered lease is set and extended in two commands that are not atomic, so K-3 on Valkey
  needs its own fix.
- `POST /internal/fire` runs a fire without the fire queue.
- Request and statement timeouts across agentd.
