# Scale hardening: dependencies

Each fact the design rests on, with its evidence. "Live" means Anthropic's hosted Claude Platform at
api.anthropic.com. Docs were read on platform.claude.com on 2026-11-19; probes ran on 2026-11-19 with `ant` 1.39.0
and made no model call. Every row is `VERIFIED`.

## Live

| # | Fact | Status | Evidence |
|---|---|---|---|
| L1 | Webhooks: "up to three delivery attempts", "jittered exponential backoff between 5 and 120 seconds", then "the event is dropped". A 3xx disables "immediately, on the first attempt"; so does an endpoint that "resolves to a non-public IP address". Sustained failure disables by "how long the endpoint has been failing without interruption, not a delivery count", with no window stated. "Events emitted while the endpoint was disabled aren't replayed." One `whsec_` secret per endpoint; no per-endpoint rate limit, rotation or backlog age is stated. | VERIFIED | `managed-agents/webhooks` |
| L2 | Rate limits are per organization, in a fixed 60 s window that starts at the window's first request. Buckets: create 300; read 1,200, shared by GETs on agents, sessions (list, retrieve, events list), vaults, deployments and deployment runs; memory stores 30,000; files about 500. Not limited: `GET /v1/environments`, `GET /v1/skills`, `POST /v1/sessions/{id}/events`. A limited route sends `anthropic-ratelimit-requests-limit`, `-remaining` and `-reset` (RFC 3339, seconds); a refusal is 429 `rate_limit_error` with `retry-after` in seconds. | VERIFIED | `managed-agents/reference#rate-limits`, `api/errors`, `api/rate-limits`; DEP-1 to DEP-5 |
| L3 | 529 `overloaded_error`; 504 `timeout_error` "when a request reaches the server's time limit before the response starts"; the limit is not stated. | VERIFIED | `api/errors` |
| L4 | "Versions are retained for 30 days after they are written; however, the recent versions of a live memory are always kept regardless of age." How many is not stated. "A store holds a maximum of 10,000 memories." | VERIFIED | `managed-agents/memory` |
| L5 | After `expires_at` a file's content is 404; its metadata "remains readable for up to 30 days" and it "continues to appear in list responses during that window". 500 MB per file. | VERIFIED | `build-with-claude/files` |
| L6 | "The server doesn't delete a session because of its age, archived or not." | VERIFIED | `managed-agents/self-hosted-data-and-access` |
| L7 | The events list takes `limit`, `order` by `processed_at` (default ascending) and an opaque `page`. | VERIFIED | `api/beta/sessions/events/list` |
| L8 | MCP output over 100,000 characters goes to a file; no reply size limit or call timeout is stated. | VERIFIED | `managed-agents/mcp-connector` |
| DEP-1 | Live reads carry `anthropic-ratelimit-requests-{limit,remaining,reset}`; one 1,200 bucket covers agents, sessions, the events list, vaults, deployments and deployment runs. | VERIFIED | Probe: `ant --debug beta:agents list`, `retrieve`, `beta:sessions list`, `beta:sessions:events list`, `beta:vaults list`, `beta:deployments list`, `beta:deployment-runs list`: limit 1200, `remaining` shared |
| DEP-2 | The window is fixed, 60 s from its first request. | VERIFIED | Probe: requests from 17:41:22 to 17:41:58 all carried `reset 17:42:22Z`; the next, at 17:43:43, carried `remaining 1199` and `reset 17:44:43Z` |
| DEP-3 | Creates have a 300 limit, with the headers even on a 400. | VERIFIED | Probe: `POST /v1/agents` with an unsupported model: 400, limit 300, remaining 299; nothing created |
| DEP-4 | Memory stores have a 30,000 limit; environments, skills, the files list and events send carry no rate-limit headers. | VERIFIED | Probe: `beta:memory-stores list` limit 30000; `beta:environments list`, `beta:skills list`, `beta:files list` and `POST /v1/sessions/{id}/events` (400 on a made-up id) carry none |
| DEP-5 | A refusal is 429 `rate_limit_error` with `retry-after`; files are limited to about 500 a minute; limits are per organization. | VERIFIED | `api/errors`, `api/rate-limits`, `managed-agents/reference#rate-limits` |

## Industry and libraries

| # | Fact | Status | Evidence |
|---|---|---|---|
| DEP-8 | Svix disables an endpoint after 120 hours of failure (`endpoint_failure_disable_after = 120`). | VERIFIED | `svix-webhooks/server/svix-server/config.default.toml`; docs.svix.com, "a period of 5 days" |
| DEP-9 | Kubernetes `terminationGracePeriodSeconds` defaults to 30 s. | VERIFIED | kubernetes.io, "Pod termination" |
| DEP-10 | `webhook-signature` may carry several space-delimited signatures. | VERIFIED | Standard Webhooks specification |
| DEP-13 | diesel-async discards a connection dropped inside a transaction. | VERIFIED | diesel-async 0.10.0 `pooled_connection/mod.rs:228` (`is_broken` checks the transaction manager); `deadpool.rs:103` drops a broken object |
| DEP-15 | Postgres uses a partial index only when the query's `WHERE` implies its predicate. | VERIFIED | postgresql.org/docs/current/indexes-partial.html |
| DEP-16 | redis-rs 1.8.2 cluster-async supports `SSUBSCRIBE` over RESP3 with a push sender, holding the node connections itself. | VERIFIED | `Cargo.lock` redis 1.8.2 with `cluster-async`; `cluster_handling/async_connection/mod.rs:50-62,203`; agentd's cluster client already uses RESP3 (`valkey.rs:94-95`) |
| DEP-17 | S3 multipart parts are 5 MiB to 5 GiB except the last, at most 10,000 parts. | VERIFIED | docs.aws.amazon.com, "Amazon S3 multipart upload limits" |
| DEP-19 | Oban's stager polls every 1,000 ms by default and notifies queues over its PubSub notifier, polling when that fails; Graphile Worker's `pollInterval` defaults to 2,000 ms. | VERIFIED | oban.hexdocs.pm/Oban.html (`stager: [interval: 1_000]`); worker.graphile.org/docs/config (`worker.pollInterval`, "Default value: `2000`") |

## agentd

| # | Fact | Status | Evidence |
|---|---|---|---|
| DEP-11 | `api_keys`, `environment_keys`, `admin_keys` and `webhook_deliveries` are on the Debezium feed. | VERIFIED | `docker/debezium/application.properties:38` |
| DEP-12 | The Kafka consumer keeps polling and pauses the partition while a record retries (durable-scheduler DS-13), so a slow handler never costs the group. | VERIFIED | `changes-kafka/src/retry.rs` |
| DEP-14 | `processed_at` and `expires_at` are written in the fixed format `%Y-%m-%dT%H:%M:%S%.6fZ`, so they sort as text. | VERIFIED | `shared/src/time.rs:40`, the one serializer for `Timestamp`; the build adds a test that both fields use it |
| DEP-18 | `webhook_deliveries` and `loop_steps` are not in Diesel's `schema.rs`; their SQL is `sql_query`. | VERIFIED | `store-postgres/src/schema.rs` |
| DEP-20 | Each delivery row is inserted once, in the commit of the status change it reports, with a random `whe_` id, so a record the feed hands over twice is still one row. | VERIFIED | `store-postgres/src/entities/webhook_deliveries.rs:26-61`, `webhooks/src/event.rs:108-111` |
