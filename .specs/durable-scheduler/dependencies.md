# Durable scheduler: dependencies

Each fact the design rests on, with its evidence. "Live" means Anthropic's hosted Managed Agents API. `VERIFIED`
rows were read in the named documentation or code; `OPEN` rows name the probe that closes them.

## Live scheduled deployments

Evidence for L1 to L4: platform.claude.com/docs/en/managed-agents/scheduled-deployments, read 2026-11-16.

| # | Fact | Status | Evidence |
|---|---|---|---|
| L1 | Live applies "jitter of up to 15 percent of the interval between runs, with a minimum of 5 seconds and a maximum of 9 minutes"; `upcoming_runs_at` shows the exact schedule. | VERIFIED | the page above |
| L2 | Unpause "resumes the schedule from the next scheduled occurrence"; "Missed triggers are not backfilled." Pause suppresses scheduled triggers from that point on. | VERIFIED | the page above |
| L3 | A rate-limited session creation is recorded at once as a `session_rate_limited_error` run without retry, and the schedule tries again at its next occurrence; other unrecoverable creation errors record a failed run and pause the deployment. | VERIFIED | the page above |
| L4 | The page says nothing about occurrences missed during an outage, any lateness window, a run overlapping a running session, or a delivery guarantee. | VERIFIED | the page above |
| B33 | Whether live's jitter only delays a run (starts at or after the listed time) or can also start it early. agentd only delays a run, as the page reads "start at or shortly after". | OPEN | Closes with a no-model probe: create deployments with a `* * * * *` cron on an archived environment, so each run fails at session creation and calls no model, and compare each run's `created_at` with its `trigger_context.scheduled_at`. |
| B34 | Live's deployment create refuses a cron that parses but never fires (`0 0 31 2 *`) with 400 `invalid_request_error` "`schedule.expression` does not produce any occurrences in the next year"; agentd answers the same. | VERIFIED | a no-model probe: `POST /v1/deployments` with that cron |

## Industry and libraries

| # | Fact | Status | Evidence |
|---|---|---|---|
| I1 | Temporal Schedules default to the overlap policy `Skip` and a catch-up window of one year (at least 10 s), and support a `jitter` on the spec. | VERIFIED | docs.temporal.io/evaluate/features/schedules |
| I2 | The Temporal Rust SDK is generally available, for Temporal Cloud and self-hosted Temporal. | VERIFIED | temporal.io/blog/build-durable-applications-rust-temporal-rust-sdk-now-generally-available (2026-09) |
| I3 | EventBridge Scheduler retries a delivery with exponential backoff until `MaximumRetryAttempts` (0 to 185) or `MaximumEventAgeInSeconds` (60 to 86,400) runs out. | VERIFIED | docs.aws.amazon.com/scheduler/latest/APIReference/API_RetryPolicy.html |
| I4 | Quartz's default misfire policy for a cron trigger fires a misfired trigger once now and then resumes its schedule; the misfire threshold defaults to 60,000 ms. | VERIFIED | quartz-scheduler.org CronTrigger javadoc (`MISFIRE_INSTRUCTION_SMART_POLICY`, `MISFIRE_INSTRUCTION_FIRE_ONCE_NOW`) |
| I5 | Oban's cron plugin never enqueues a duplicate job across nodes; River inserts periodic jobs in the same Postgres transaction as other writes and supports unique jobs. | VERIFIED | hex.pm/packages/oban README; github.com/riverqueue/river README |
| I6 | librdkafka removes a consumer from its group when the application does not poll within `max.poll.interval.ms`, default 300,000 ms; a paused partition delivers nothing while the consumer keeps polling. | VERIFIED | librdkafka `CONFIGURATION.md`; rdkafka `Consumer::pause` |
