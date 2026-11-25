# Operations

How to run agentd and what to watch. Every setting is in `configuration.md`; every metric, span and log line in
`observability.md`; the change feed's records and slot runbook in `history.md`. agentd exports metrics over OTLP and chooses no
monitoring backend: the alarms below are for whatever backend the deployment runs.

## Roles

One binary serves three roles, chosen with `AGENTD_ROLE`:

| Role | Runs | Scale on |
|---|---|---|
| `api` | the HTTP API, event streams, event retention | request rate and open streams |
| `loop` | loop workers only; every route answers 404 | `agentd.loop.steps_due` |
| `all` | both, for a single replica or a small deployment | either |

Once an hour, one replica (under the advisory lock `cleanup_sweep`) removes every secret no `credentials` row
names and every blob no `files`, `skill_versions` or `memory_versions` row names, leaving anything written in the
last 24 hours and any key not in the form agentd writes (a typed id for each vault, credential, file, skill,
memory store and version): it retries a removal that failed after a delete committed. The first run is an hour
after the process starts. Each run logs `sweep: removed the secrets
and blobs no row names` at `INFO` with the counts, and a failed run `warn`s and is tried again the next hour.

Every role runs the fire workers and the change feed's handlers (schedules, webhooks), and either ticks
the built-in scheduler (`AGENTD_SCHEDULER=postgres`) or runs agentd's Temporal worker (`temporal`). Replicas hold nothing between requests or steps, so any number of each role run side by side
and any one of them can stop at any time.

## A deployment

- Postgres with `wal_level=logical`, migrated with Flyway from `crates/adapters/store-postgres/migrations`.
- Valkey (standalone, or a cluster with `AGENTD_VALKEY_CLUSTER=true`) for the signals between replicas and the
  model budget.
- The change feed: Debezium Server, one instance, into Kafka (`AGENTD_KAFKA_BROKERS`) or Valkey Streams, set up
  as `configuration.md`'s change feed section says.
- S3 for blobs. Vault credentials are sealed in Postgres under `AGENTD_SECRET_KEYS` by default, or kept in
  Secrets Manager or SSM Parameter Store under a KMS key (`AGENTD_SECRETS`).
- The keys from the environment (`AGENTD_API_KEY_PEPPERS`, `AGENTD_WORK_TOKEN_KEYS`, `AGENTD_PLACEHOLDER_KEYS`,
  `AGENTD_SECRET_KEYS` on the Postgres store), a model provider key, `AGENTD_API_BASE_URL`, and
  `AGENTD_WEBHOOK_ENDPOINTS` for webhooks.
- At least two `api` replicas behind the load balancer and two `loop` replicas, or two `all` replicas.

A `self_hosted` environment's tools run on the customer's worker (`ant beta:worker poll` or the SDK's
`EnvironmentWorker`), which polls the environment's work queue.

## Signals

| Signal | Threshold | What it means, and what to do |
|---|---|---|
| `agentd.loop.steps_due` (max across replicas) | above 0 for 5 minutes while the loop replicas' memory is at `AGENTD_MEMORY_TARGET` | steps wait for memory: add `loop` replicas, or raise their memory limit |
| `agentd.loop.step_wait` p95 | above 5 s for 10 minutes | the same, seen as latency; check the workspace cap (`AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE`) when one workspace dominates |
| `agentd.loop.model_call_duration` with outcome `error` | above 5% of calls for 10 minutes | the provider fails or rate-limits: check its status, `AGENTD_MODEL_RPM` and `AGENTD_MODEL_ITPM` |
| `agentd.loop.turns` with stop reason `retries_exhausted` | above 1% of turns for 15 minutes | turns end on model errors: the same as above |
| `agentd.loop.compactions` with outcome `failed` or `no_summary` | above 10% of compactions for 30 minutes | threads grow to the window with no summary: check the provider's compaction support for the model |
| `agentd.loop.mcp_calls` with outcome `unreachable` | above 10% for 15 minutes | MCP servers fail: usually one tenant's server; check the `session.error` events and the `is_error` `agent.mcp_tool_result` events for its name |
| `http.server.request.duration` 5xx ratio | above 1% for 5 minutes | the API fails: read the `request failed` log lines and their spans |
| `db.client.connection.pending_requests` | above 0 for 5 minutes | the pool is saturated: raise `AGENTD_DB_POOL_SIZE` or add replicas within the database's connection limit |
| Slot, Debezium health, feed backlog | as `configuration.md`'s change feed section says | the change feed stalls: schedules and webhooks wait until it moves again |
| `agentd.feed.slot.lost` | 1, or absent from every replica for 5 minutes | the slot is invalidated (`wal_status = 'lost'`) and the feed has a gap: follow `history.md` ("After a lost slot"). A slot that does not exist yet, on a new deployment, is no gap only if nothing committed before Debezium Server first made it. Absent: no replica can read the slot |
| `webhook_deliveries` rows with `dropped_at` set | any for a configured endpoint | an endpoint refused three attempts: check the endpoint and its secret |
| `agentd.retention.last_success` | older than 15 minutes while retention is set | retention stopped: read its `warn` lines |
| `agentd.scheduler.fires.due` (max across replicas) | above 0 for 5 minutes | due fires wait: the workers are failing or behind; read the `fires:` log lines and `agentd.scheduler.fires` with outcome `retried` |
| `agentd.scheduler.fire.lag` p99 | over 60 s | fires start late: check the database's load and the fire handlers' `warn` lines |
| `agentd.scheduler.fires` with outcome `skipped` or `dropped` | any | an occurrence did not run: `skipped` ran out of time before its following occurrence; `dropped` has no handler or an unreadable spec (its `ERROR` line names the target) |
