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

Each replica opens up to `AGENTD_DB_POOL_SIZE` (16) connections per pool, a primary and an optional read pool,
so Postgres `max_connections` must be at least replicas × pools × `AGENTD_DB_POOL_SIZE` + 20 (replicas × 32 + 20
at the default with a read pool), or a pooler (PgBouncer in transaction mode) sits in front.

A `self_hosted` environment's tools run on the customer's worker (`ant beta:worker poll` or the SDK's
`EnvironmentWorker`), which polls the environment's work queue.

## Shutdown and deadlines

At SIGTERM a replica stops accepting connections, stops claiming steps and webhook deliveries, and drains its
open requests, running steps and webhook sends in parallel inside one grace, `AGENTD_SHUTDOWN_GRACE_MS`
(25 s). Set the platform's kill timeout above it (Kubernetes' `terminationGracePeriodSeconds` defaults to
30 s). When the grace ends, the steps still running are released, due at once, so another replica takes them
(`agentd.loop.released`); open event streams are dropped and clients reconnect. A webhook claim left behind
expires in 60 s and another replica sends the delivery.

Every statement runs under `AGENTD_DB_STATEMENT_TIMEOUT_MS` (30 s) and every transaction under a 60 s idle
timeout. A response that has not started within `AGENTD_REQUEST_TIMEOUT_MS` (60 s; files 15 minutes) is
504 `timeout_error`, its transaction rolled back; a write that committed before then stays, so a 504 means the
outcome is unknown, as on live. When no Postgres connection comes free within `AGENTD_DB_POOL_WAIT_MS`, a
request answers 529 `overloaded_error`. Each S3 attempt is bounded to 30 s and each operation to 50 s.

## Webhook endpoints

An endpoint that answers 3xx, resolves to a non-public address, or fails every attempt for 120 h is disabled
and logs one `ERROR` naming the organization, endpoint and reason. Fix the endpoint, then run
`agentd webhooks enable <org_id> <endpoint_id>`, or change its URL or secret in `AGENTD_WEBHOOK_ENDPOINTS` and
restart. Events emitted while it was disabled are not sent.

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
| `agentd.webhooks.deliveries` with outcome `dropped` | above 0 for a configured endpoint | an endpoint refused three attempts, or is disabled: check the endpoint, its secret and `agentd.webhooks.endpoints_disabled` |
| `agentd.webhooks.oldest_due_age` (max across replicas) | above 5 minutes | due deliveries wait for a sender: the receivers are slow; add replicas, each adds 256 senders |
| `agentd.webhooks.endpoints_disabled` (max across replicas) | above 0 | an endpoint is disabled: read its `ERROR` line, fix it, then `agentd webhooks enable` |
| `agentd.http.rate_limit_unchecked` | any increase over 5 minutes (it is a counter) | Valkey does not answer rate checks in 200 ms, so requests are not limited: check Valkey |
| `agentd.retention.last_success` | older than 15 minutes while retention is set | retention stopped: read its `warn` lines |
| `agentd.scheduler.fires.due` (max across replicas) | above 0 for 5 minutes | due fires wait: the workers are failing or behind; read the `fires:` log lines and `agentd.scheduler.fires` with outcome `retried` |
| `agentd.scheduler.fire.lag` p99 | over 60 s | fires start late: check the database's load and the fire handlers' `warn` lines |
| `agentd.scheduler.fires` with outcome `skipped` or `dropped` | any | an occurrence did not run: `skipped` ran out of time before its following occurrence; `dropped` has no handler or an unreadable spec (its `ERROR` line names the target) |
