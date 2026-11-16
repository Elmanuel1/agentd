# Durable scheduler: decisions

agentd's built-in scheduler never loses a fire: every due occurrence is a row in `fire_claims` before anything runs
it, and every replica runs due rows from there ([ADR 0001](adr/0001-durable-fire-queue.md)). Temporal is an optional
scheduler sink behind `SchedulerPort`. "Live" means Anthropic's hosted Managed Agents API. Each row names who
decided and when.

## Asked and answered

| # | Decision | Source |
|---|---|---|
| D1 | A fire stays runnable until its schedule's following occurrence is due; after downtime only each schedule's latest missed occurrence fires, once, with no backfill of older ones. Live documents no window for an outage (L4). | User, 2026-11-16 ("Until next occurrence") |
| D2 | Temporal is an optional scheduler sink in agentd: a `SchedulerPort` adapter over Temporal Schedules on the Rust SDK, selected by `AGENTD_SCHEDULER=temporal`, whose actions write occurrences into the fire queue. The adapter crate and the `temporalio-client` and `temporalio-sdk` dependencies are approved. | User, 2026-11-16 ("Yes, as PR 8") |
| D3 | A done fire records why it finished in `fire_claims.outcome`: `ran`, `skipped` or `dropped`. | User, 2026-11-16 ("yes") |
| D4 | The plan of PRs (PR 0 to PR 8), the V16 columns (`spec`, `due_at`, `outcome`) and the V17 indexes are approved. | User, 2026-11-16 ("Approve plan") |

## Matched to live

| # | Decision | Source |
|---|---|---|
| D5 | A fire starts after its listed time by a delay of up to 15 percent of the interval to the schedule's following occurrence, at least 5 s and at most 9 minutes; `upcoming_runs_at` and `trigger_context.scheduled_at` keep the exact time. | Live, L1 (`feedback-match-live-dont-ask`) |
| D6 | An unpaused schedule resumes from its next occurrence after the unpause; a time it missed while paused does not fire. | Live, L2 |
| D7 | A fire whose session cannot be created records the failed run and is not retried; only failures that record no run are retried. | Live, L3 |

## Taken from the standing rules

| # | Decision | Source |
|---|---|---|
| D8 | The fire queue lives in Postgres, in `fire_claims`, claimed with `FOR UPDATE SKIP LOCKED` under a lease. No fire is held in a replica's memory. | Runtime state lives in the database (design skill); user brief, 2026-11-16 |
| D9 | Schedule changes still reach a sink through the change feed, an after-effect of the deployment write; fires an external engine delivers on `agentd.fires` enter the same queue. | `feedback-wal-for-after-effects`; runtime-parity D50 |
| D10 | agentd exports the scheduler's metrics and documents each signal's threshold; it defines no alarm. | Standing answer, 2026-11-11 ("We just export metrics.") |
| D11 | The scheduler's settings are environment variables. | Standing answer, 2026-11-11 |
| D12 | Done `fire_claims` rows are deleted 30 days after their occurrence. The table coordinates fires; the history of each fire is its deployment run. | Coordination tables are exempt from revisions (user, 2026-11-05, `project-agentd-history`) |
| D13 | Log levels: a skipped fire is `WARN` (handled, degraded); an invalid schedule change or an unreadable schedule is `ERROR` (a person must fix its data). | Standing answer on log levels, 2026-11-12 |
