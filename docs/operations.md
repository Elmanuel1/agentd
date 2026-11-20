# Operations

How to run agentd and what to watch. Every setting is in `configuration.md`; every metric, span and log line in
`observability.md`; resource history's runbooks in `history.md`. agentd exports metrics over OTLP and chooses no
monitoring backend: the alarms below are for whatever backend the deployment runs.

## Roles

One binary serves three roles, chosen with `AGENTD_ROLE`:

| Role | Runs | Scale on |
|---|---|---|
| `api` | the HTTP API, event streams, event retention | request rate and open streams |
| `loop` | loop workers only; every route answers 404 | `agentd.loop.steps_due` |
| `all` | both, for a single replica or a small deployment | either |

Every role runs the fire workers and the change feed's handlers (schedules, webhooks, history), and either ticks
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
- On Kubernetes, the Helm chart in `deploy/helm/agentd`; `kubernetes.md` has its values and a local kind setup.

A `self_hosted` environment's tools run on the customer's worker (`ant beta:worker poll` or the SDK's
`EnvironmentWorker`), which polls the environment's work queue.

## Signals

| Signal | Threshold | What it means, and what to do |
|---|---|---|
| `agentd.loop.steps_due` (max across replicas) | above 2× `AGENTD_STEPS_PER_WORKER` × loop replicas for 5 minutes | steps wait for a worker: add `loop` replicas |
| `agentd.loop.step_wait` p95 | above 5 s for 10 minutes | the same, seen as latency; check the workspace cap (`AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE`) when one workspace dominates |
| `agentd.loop.model_call_duration` with outcome `error` | above 5% of calls for 10 minutes | the provider fails or rate-limits: check its status, `AGENTD_MODEL_RPM` and `AGENTD_MODEL_ITPM` |
| `agentd.loop.turns` with stop reason `retries_exhausted` | above 1% of turns for 15 minutes | turns end on model errors: the same as above |
| `agentd.loop.compactions` with outcome `failed` or `no_summary` | above 10% of compactions for 30 minutes | threads run on cleared prompts only: check the provider's compaction support for the model |
| `agentd.loop.mcp_calls` with outcome `unreachable` | above 10% for 15 minutes | MCP servers fail: usually one tenant's server; check the `session.error` events and the `is_error` `agent.mcp_tool_result` events for its name |
| `http.server.request.duration` 5xx ratio | above 1% for 5 minutes | the API fails: read the `request failed` log lines and their spans |
| `db.client.connection.pending_requests` | above 0 for 5 minutes | the pool is saturated: raise `AGENTD_DB_POOL_SIZE` or add replicas within the database's connection limit |
| Slot, Debezium health, feed backlog | as `configuration.md`'s change feed section says | the change feed stalls: schedules, webhooks and history wait until it moves again |
| `agentd.feed.slot.lost` | 1, or absent from every replica for 5 minutes | the slot is invalidated (`wal_status = 'lost'`) and history has a gap: re-baseline as `history.md` says ("After a lost slot"). A slot that does not exist yet, on a new deployment, is no gap only if nothing committed before Debezium Server first made it. Absent: no replica can read the slot |
| `agentd.history.apply_delay` p99 | over 5 minutes | history lags its commits: check the feed backlog and the `history:` log lines |
| `webhook_deliveries` rows with `dropped_at` set | any for a configured endpoint | an endpoint refused three attempts: check the endpoint and its secret |
| `agentd.retention.last_success` | older than 15 minutes while retention is set | retention stopped: read its `warn` lines |
| `agentd.scheduler.fires.due` (max across replicas) | above 0 for 5 minutes | due fires wait: the workers are failing or behind; read the `fires:` log lines and `agentd.scheduler.fires` with outcome `retried` |
| `agentd.scheduler.fire.lag` p99 | over 60 s | fires start late: check the database's load and the fire handlers' `warn` lines |
| `agentd.scheduler.fires` with outcome `skipped` or `dropped` | any | an occurrence did not run: `skipped` ran out of time before its following occurrence; `dropped` has no handler or an unreadable spec (its `ERROR` line names the target) |
