# Requirements: scale hardening

**Date:** 2026-11-19
**Format:** EARS (Easy Approach to Requirements Syntax). Each requirement uses one pattern, named in column P:

| P | Pattern | Template |
|---|---|---|
| U | Ubiquitous | The `<system>` shall `<response>`. |
| E | Event-driven | When `<trigger>`, the `<system>` shall `<response>`. |
| S | State-driven | While `<state>`, the `<system>` shall `<response>`. |
| W | Unwanted behaviour | If `<condition>`, then the `<system>` shall `<response>`. |
| O | Optional feature | Where `<feature>`, the `<system>` shall `<response>`. |

**Systems:**

| System | What it is | Rows here |
|---|---|---|
| the edge | agentd's HTTP layer (auth, limits, deadline, files) | SH-11 to SH-16, SH-18, SH-27, SH-45, SH-46 |
| the webhook sender | the feed handler that wakes, and the claim loop on every instance | SH-1 to SH-4, SH-20, SH-29 to SH-34, SH-43, SH-52 |
| the change feed | the Kafka or Valkey Streams consumer of Debezium's records | SH-5, SH-6 |
| the event log | the `session_events` store behind `SessionEventLog` | SH-22, SH-23 |
| the loop | the engine worker that claims and runs steps | SH-7 to SH-9, SH-24 to SH-26, SH-47, SH-48, SH-53 |
| the sweeper | the retention job | SH-37 to SH-41 |
| the blob store | S3 or the local store | SH-21 |
| agentd | the process as a whole (statements, keys, lists, shutdown, metrics) | SH-10, SH-17, SH-19, SH-28, SH-36, SH-42, SH-44 |
| the operator command | `agentd webhooks` | SH-35 |

**Terms:**

- **Live**: Anthropic's hosted Claude Platform at api.anthropic.com, as its docs and no-model probes show.
- **Delivery**: one `webhook_deliveries` row, one event to one endpoint.
- **Due**: a delivery with `next_attempt_at <= now()`, not delivered, not dropped, not claimed.
- **Claim**: an `UPDATE` that sets `claimed_until` (deliveries) or a lease (steps), so only one replica acts.
- **Sender**: one send in flight. An instance has 256.
- **Wake**: a Valkey message that tells every instance new deliveries are due.
- **Keyset page**: a page that starts after the previous page's last sort key instead of skipping `OFFSET` rows; the
  token stays an opaque `page_<base64>`.
- **Sweep**: deleting rows past their retention in batches of 1,000, one replica at a time.
- **Grace**: the time between SIGTERM and the forced stop; agentd finishes everything inside it.

Source decisions: [decisions.md](decisions.md). Evidence: [dependencies.md](dependencies.md). The decision record:
[ADR 0001](adr/0001-webhooks-sent-from-rows.md). SH-3 supersedes runtime-parity H-3; SH-1 to SH-4 narrow H-4 and
D51 ([runtime-parity requirements](../runtime-parity/requirements.md)). SH-41 bounds how E-2 deletes. IDs SH-49 to
SH-51 are not used.

**Build order:** the spec, then code PR 1 (commits 1 to 13), then code PR 2 (commits 1 to 6), stacked with `gh stack`.

## 1. Webhooks

| ID | P | Requirement |
|---|---|---|
| SH-1 | E | When the change feed hands the webhook sender a new delivery, the webhook sender shall publish one wake to every instance and return without sending. |
| SH-2 | U | The webhook sender shall send a delivery only after claiming it for 60 s, shall start its POST only while at least 15 s of that claim remain, and shall send nothing for a delivery another replica holds or has finished. |
| SH-3 | W | If a delivery is not answered 2xx, then the webhook sender shall set its next attempt by exponential backoff with jitter, 15 to 30 s after the first attempt and 30 to 60 s after the second, and drop it after its third attempt (L1 "jittered exponential backoff"; supersedes H-3's fixed 31 s). |
| SH-4 | E | When an instance is woken, and at least every second, the webhook sender shall claim with `SKIP LOCKED` up to its free senders (at most 100) of due deliveries, leaving out endpoints whose breaker is open on it, at most once every 50 ms. |
| SH-20 | U | The webhook sender shall send through the egress guard with `user-agent: Claude-Platform`, reusing a client per host for 5 minutes. |
| SH-29 | W | If an endpoint resolves to a non-public address, then the webhook sender shall not send to it and shall disable it. |
| SH-30 | W | If an endpoint answers 3xx, then the webhook sender shall disable it. |
| SH-31 | W | If every attempt to an endpoint has failed for 120 h, then the webhook sender shall disable it. |
| SH-32 | E | When an endpoint answers 2xx, the webhook sender shall clear its failing time. |
| SH-33 | S | While an endpoint's breaker is open on a replica, the webhook sender on that replica shall claim none of its deliveries. |
| SH-34 | S | While an endpoint is disabled, the webhook sender shall drop its deliveries without sending them, on every replica within 10 s of the disable. |
| SH-35 | E | When an operator runs `agentd webhooks enable <org> <endpoint>`, the operator command shall re-enable the endpoint and clear its failing time. |
| SH-36 | E | When agentd starts with an endpoint whose URL or secret digest differs from the stored one, agentd shall re-enable it and store the new digest. |
| SH-43 | E | When the webhook sender disables an endpoint, the webhook sender shall log one `ERROR` naming the org, endpoint and reason. |
| SH-52 | U | The webhook sender shall deliver more webhooks a second with each added instance, and shall send a delivery again only when its outcome was not recorded before its claim expired. |

## 2. The change feed

| ID | P | Requirement |
|---|---|---|
| SH-5 | U | The change feed shall extend an ordered stream's lease only while it holds it, before each entry and every 10 s while an entry runs, and shall stop the batch when the lease is lost. |
| SH-6 | U | The change feed shall read at most 16 entries of an unordered stream at a time and handle them concurrently. |

## 3. The loop

| ID | P | Requirement |
|---|---|---|
| SH-7 | E | When a replica receives SIGTERM, the loop shall stop claiming steps and wait for running steps until the grace ends. |
| SH-8 | E | When the grace ends, the loop shall release every step it still holds so that it is due at once. |
| SH-9 | W | If a step fails, then the loop shall make it due again after a random wait from 0 to the smaller of 5 minutes and 5 s × 2^(failures before this one), and shall reset the count when the step commits. |
| SH-24 | U | The loop shall wake a watch only for its own session's events, and an idle claimer only when a step becomes due. |
| SH-25 | U | The loop shall reuse an MCP server's HTTP client for 5 minutes. |
| SH-26 | W | If an MCP reply exceeds 16 MiB, then the loop shall fail the call as an unreachable server. |
| SH-47 | W | If a model provider fails 5 calls in a row from one replica with a retryable error, then the loop shall make no call to it from that replica for 30 s, or until its `retry-after` if later, and then let one call through. |
| SH-48 | S | While a provider's breaker is open, the loop shall wait in each step's retry without spending a try, inside the step's deadline. |
| SH-53 | W | If a step is over its workspace budget, then the loop shall make it due after the time left in the budget window plus a random 0 to 60 s. |

## 4. The edge

| ID | P | Requirement |
|---|---|---|
| SH-11 | W | If a request has not started its response within `AGENTD_REQUEST_TIMEOUT_MS` (60 s; 15 min for files), then the edge shall answer 504 `timeout_error` and roll back the request's open transaction. |
| SH-12 | U | The edge shall not apply the request deadline to a stream's body or to health routes. |
| SH-13 | W | If an organization sends more than 300 create, 1,200 read, 30,000 memory-store or 500 files requests in one 60 s window (the routes in the rate-limit table), then the edge shall answer 429 `rate_limit_error` with `retry-after` set to the seconds left in the window. |
| SH-14 | U | The edge shall send `anthropic-ratelimit-requests-limit`, `-remaining` and `-reset` on every create, read and memory-store response. |
| SH-15 | W | If Valkey does not answer a rate check within 200 ms, then the edge shall admit the request and count it as unchecked. |
| SH-16 | U | The edge shall resolve a key it resolved within the last 60 s without reading Postgres. |
| SH-18 | W | If a revoke's message is lost, then the edge shall refuse the key within 60 s of the revoke. |
| SH-21 | U | The blob store shall bound each S3 attempt to 30 s and each operation to 50 s. |
| SH-27 | U | The edge shall upload and download a file holding at most one 8 MiB part in memory. |
| SH-45 | S | While an expired file is less than 30 days past `expires_at`, the edge shall list it and return its metadata (L5). |
| SH-46 | E | When a client asks for an expired file's content, the edge shall answer 404 (L5). |

## 5. Postgres and keys

| ID | P | Requirement |
|---|---|---|
| SH-10 | U | agentd shall run every Postgres statement under a 30 s `statement_timeout` and every transaction under a 60 s `idle_in_transaction_session_timeout`. |
| SH-17 | E | When the change feed delivers a key's revoke, agentd shall refuse the key on every replica within 1 s. |
| SH-19 | U | agentd shall open at most `AGENTD_DB_POOL_SIZE` (16) connections per Postgres pool (S-8). |

## 6. The event log and lists

| ID | P | Requirement |
|---|---|---|
| SH-22 | U | The event log shall answer tool-call answers, queued events, stream starts and the idle-send guard from the indexes named in the design, reading only matching index entries. |
| SH-23 | U | The event log shall read the events list in index order from the previous page's last `(processed_at, seq)`, unprocessed events last. |
| SH-28 | U | agentd shall page sessions, work items, agents, memory versions, deployment runs and agent versions in SQL from the previous page's last row, and memories in SQL at depth 0. |

## 7. Retention

| ID | P | Requirement |
|---|---|---|
| SH-37 | U | The sweeper shall run one batch at a time across the cluster, under a transaction-level advisory lock. |
| SH-38 | U | The sweeper shall delete deliveries 24 h after delivery and 30 days after they are dropped. |
| SH-39 | U | The sweeper shall delete memory versions 30 days after they are written, except the current version of a live memory (L4). |
| SH-40 | U | The sweeper shall delete an expired file's blob, then its row, 30 days after it expires (L5). |
| SH-41 | O | Where event retention is set, the sweeper shall delete at most 10,000 events per statement. |

## 8. Shutdown and metrics

| ID | P | Requirement |
|---|---|---|
| SH-42 | E | When a replica receives SIGTERM, agentd shall stop the webhook claim loop, the sweeper and new HTTP connections, and drain sends, steps and requests in parallel within `AGENTD_SHUTDOWN_GRACE_MS` (25 s). |
| SH-44 | U | agentd shall export the metrics in the metrics table. |

## Metrics

OTLP; each threshold is in the docs; agentd defines no alarm (D11).

| Metric | Kind | Labels | Threshold in docs |
|---|---|---|---|
| `agentd.webhooks.deliveries` | counter | `outcome` = delivered, retried, dropped | dropped above 0 for a configured endpoint |
| `agentd.webhooks.oldest_due_age` | gauge | | above 5 min |
| `agentd.webhooks.endpoints_disabled` | gauge | `reason` | above 0 |
| `agentd.webhooks.breaker_opened` | counter | | |
| `agentd.webhooks.claims` | counter | `instance` | |
| `agentd.http.rejected` | counter | `reason` = rate_limited, timeout | |
| `agentd.http.rate_limit_unchecked` | counter | | above 0 for 5 min |
| `agentd.keys.cache` | counter | `result` = hit, miss | hit ratio below 0.9 |
| `agentd.loop.released` | counter | | |
| `agentd.model.breaker_opened` | counter | `provider` | |
| `agentd.loop.step_failures` | counter | | |
| `agentd.sweeps.deleted` | counter | `table` | |

`oldest_due_age` is one `SELECT min(next_attempt_at)` on the due index per replica every 10 s.

## Industry practice

| Problem | Standard pattern (source) | This design |
|---|---|---|
| Webhook fan-out that one slow receiver must not stall | Svix, Stripe: deliveries are durable rows; a worker pool; retries scheduled in the store with jittered backoff (docs.svix.com/retries) | Followed. Svix itself is not used (D5). Svix's 8-attempt schedule is not used: live's 3 attempts win (D3). |
| A work queue in Postgres | `SELECT ... FOR UPDATE SKIP LOCKED` (Postgres docs, "The Locking Clause"; pg-boss, Oban) | Followed for the webhook claim on every instance, with `FOR NO KEY UPDATE`, the weaker lock for a row that is only updated (sql-transactions rule 19). |
| Waking a Postgres queue | A wake signal in front of a fixed poll (Oban's stager, 1 s; Graphile Worker, 2 s; DEP-19) | Followed: a Valkey wake and a 1 s poll (D18). |
| Endpoint health | Svix `endpoint_failure_disable_after`, 120 h (DEP-8); circuit breaker (Nygard, *Release It!*) | Followed: live's disable rules, Svix's window where live states none, a breaker per replica (D4). |
| A shared upstream under overload | A circuit breaker per dependency (Nygard; Envoy outlier detection); retry only idempotent calls (AWS Builders' Library, "Timeouts, retries, and backoff with jitter") | Followed: a breaker per model provider per replica that makes callers wait, not fail. MCP and token refresh keep live's retry at the next turn (D17). |
| A lease in Redis | Redis, "Distributed locks with Redis": compare-and-set release and extend, extend while working | Followed: one Lua compare-and-expire, run every 10 s while an entry runs. |
| Draining on deploy | Sidekiq shutdown; Kubernetes `terminationGracePeriodSeconds`, 30 s (DEP-9) | Followed: everything drains in parallel inside 25 s. |
| Retrying a failed job | Capped exponential backoff with full jitter (AWS Architecture Blog, "Exponential Backoff And Jitter") | Followed: 5 s doubling to 5 min. |
| Bounding work | Postgres `statement_timeout`; a deadline per request (Google SRE book, "Handling Overload") | Followed. |
| API rate limits | A counter per tenant in Redis with `retry-after` (Stripe, "Scaling your API with rate limiters") | Followed in live's shape: a fixed 60 s window per organization and bucket (DEP-1, DEP-2), not a token bucket. |
| Auth cache | A short-lived in-process cache with explicit invalidation (Kubernetes webhook token authenticator cache) | Followed: 60 s, invalidated from the change feed (D8). |
| Paging large tables | Keyset pagination (use-the-index-luke.com/no-offset) | Followed for large lists. |
| Pub/sub fan-out | A channel per entity; sharded pub/sub on a cluster (Valkey `SSUBSCRIBE`) | Followed. |
| Large uploads | S3 multipart, parts of 8 MiB, an `AbortIncompleteMultipartUpload` lifecycle rule (Amazon S3 User Guide) | Followed. |
| Growth of coordination rows | Delete done rows after a window in small batches (Oban Pruner) | Followed: one sweeper, batches of 1,000. |

## Design

### Webhook sender (W1 to W4, R1)

- **One send path, on every instance.** Webhooks are sent only from the table. The change feed does not send: on a
  `webhook_deliveries` insert its handler publishes one wake on Valkey `agentd:webhooks` and returns, so the feed is
  never held. On a wake, and every 1 s as a backstop, each instance claims due rows and sends them. Postgres is the
  central coordinator: `FOR NO KEY UPDATE SKIP LOCKED` gives each row to exactly one instance, so adding instances adds
  sending capacity, with no shared counter and no number to agree on (D1).
- **Claim**, one short transaction: select up to the instance's free senders (at most 100) of due rows, leaving out
  endpoints whose breaker is open on this instance, `ORDER BY next_attempt_at ... FOR NO KEY UPDATE SKIP LOCKED`; set
  `claimed_until = now() + 60 s` and `attempt = attempt + 1` on them in one `UPDATE`; commit; then send each in the
  background. Another instance skips the locked rows and takes others. A row whose endpoint is disabled is marked
  dropped without a send (live does not replay).
- **Two instances never hold one row.** Three guards, each enough on its own for its case:
  - The lock: the `SELECT ... FOR NO KEY UPDATE SKIP LOCKED` and the `UPDATE` run in one transaction, so no other claim can
    read the row between them.
  - The `UPDATE` repeats the conditions (not delivered, not dropped, unclaimed or claim expired) and the sender sends
    only the rows it returns. A row another instance claimed in between is simply not updated, so it is not sent.
  - Every time is Postgres `now()`, never an instance's clock, so clock skew cannot make a live claim look expired.
- **A claim outlives its send.** The instance takes its own deadline as 60 s from just before the claim statement
  (on its monotonic clock, so it is never later than the row's `claimed_until`), and starts a POST only while at least
  15 s remain: the 10 s send timeout plus 5 s spare. A row with less left is released unsent (`claimed_until = NULL`,
  `attempt` put back), so an instance that stalled cannot POST into another instance's claim.
- **Wakes are folded**: an instance runs at most one claim every 50 ms; wakes that arrive in between are folded into
  the next claim. So a new delivery goes out within about 50 ms, and the claim rate stays bounded however many rows
  arrive.
- **Senders**: 256 in flight per instance, a bound on one process, not a fairness limit. No per-organization cap for
  now (D1); follow-up F-7 adds a fair share in the claim query once the load test gives numbers.
- **Send.** Through `Guard` (resolve, refuse a non-public address, pin it), `user-agent: Claude-Platform`, a cached
  client per host (M1), 10 s timeout, 3 s connect, no redirects.
- **Outcome**, fenced on `attempt`: 2xx sets `delivered_at`; otherwise attempt 3 sets `dropped_at`, and attempts 1
  and 2 set `next_attempt_at` by exponential backoff with jitter (SH-3, L1): the first wait is 15 to 30 s, the second
  30 to 60 s (a 30 s base doubled per attempt, half fixed and half random), inside live's 5 to 120 s. Every outcome
  records `last_status` and `last_error`.
- **No double sends**: the rows are written once, in the commit of the change they report (`webhook_deliveries.rs:46-61`),
  so a record the feed hands over twice is still one row; and only the instance holding a row's claim sends it, and
  only while that claim has 15 s left. A
  delivery whose outcome is not recorded before its claim expires is the one duplicate left (the instance crashed
  after the POST, or the outcome write failed or ran late): the claim expires after 60 s and another instance sends
  it again with the same `webhook-id`, which is live's documented at-least-once delivery.
- **Breaker**, per replica in memory (design skill exception): 5 failures in a row open it for 30 s. While open, the
  endpoint is skipped: nothing is claimed for it, so no attempt is spent without a POST. After 30 s one send is let
  through; a 2xx closes it, a failure opens it again.
- **Auto-disable**, shared in `webhook_endpoint_states`:
  - a 3xx or a non-public address disables at once (L1);
  - every failure sets `failing_since` if it is empty, and disables when `failing_since` is older than 120 h (Svix's
    default; live states no window);
  - a 2xx clears `failing_since`; each replica knows which endpoints are failing from the state read below, so only
    a 2xx to a failing endpoint costs a statement;
  - every replica reads the failing and disabled sets every 10 s, so a disable reaches all replicas within 10 s;
  - disabling logs `ERROR` once (a person must act) with org, endpoint and reason;
  - re-enable (D6): `agentd webhooks enable <org> <endpoint>`, or a change to the endpoint's URL or secret in
    `AGENTD_WEBHOOK_ENDPOINTS`, seen at boot by its SHA-256 digest (the secret itself is not stored).
- **Shutdown.** At SIGTERM the instance stops claiming; sends in flight finish within the grace; a claim left behind
  expires in 60 s and another instance sends it.
- **Not added, with reasons**: a per-endpoint rate limit (live states none); secret rotation with several signatures (live gives one secret per endpoint; Standard Webhooks
  allows several, so it stays a follow-up F-5 if live adds it); a maximum backlog age (every delivery ends after 3
  attempts; a row that waits for a sender is watched by the `oldest_due_age` metric instead); honouring a receiver's
  `retry-after` (live's schedule is fixed); replay of dropped events (live does not).

### Valkey lease (V1, V2)

- Extend is one Lua script: `if GET == me then PEXPIRE ... return 1 else return 0`. Taking stays `SET NX PX`.
- The ordered pass extends before each entry, and a ticker extends every 10 s (a third of `LEASE_MS` 30 s) while the
  entry runs. On a lost lease the entry's future is dropped and the batch stops; the new holder redoes the entry
  (handlers are idempotent, CH-4).
- Unordered: read 16 at a time and handle them concurrently; reclaim idle stays 60 s.

### Shutdown (SD1)

- At SIGTERM agentd starts, at the same time: the loop stops claiming; the webhook claim loop stops; the sweeper stops after its batch; HTTP stops accepting and drains. All share one deadline,
  `AGENTD_SHUTDOWN_GRACE_MS` (25,000), so the total is 25 s, not their sum.
- At the deadline the loop aborts the steps still running and releases them in one statement, so another replica
  takes them at once. HTTP's drain moves from 10 s to the same deadline.

### Step backoff (S1)

- V25: `failures bigint not null default 0` on `loop_steps` (bigint, as `attempts`).
- `defer_failed` is one `UPDATE` that computes the wait from the row's old value (Postgres `SET` reads the old row):
  `due_at = now() + random() * least(300, 5 * 2 ^ failures) * interval '1 second'`, `failures = failures + 1`. The
  first failure waits up to 5 s (today's base), the second up to 10 s, capped at 5 min. A commit sets `failures = 0`
  in its existing `UPDATE`. The claim does not need to return `failures`.

### Budget jitter (BU1, SH-53)

- `Admit::Later(left)` becomes `left + random(0 to 60 s)`.

### Timeouts (T1 to T3)

- Each new pool connection runs `SET statement_timeout = <AGENTD_DB_STATEMENT_TIMEOUT_MS>` and
  `SET idle_in_transaction_session_timeout = '60s'` once, in the pool's `custom_setup`.
- A deadline middleware (axum `from_fn` with `tokio::time::timeout`; tower's `timeout` feature is off, so no new
  feature) bounds the time to the response head: `AGENTD_REQUEST_TIMEOUT_MS` (60,000), 15 min for files (next to
  `BodyLimit`). Over it: 504 `timeout_error` (L3), new `ErrorType::Timeout`. A stream returns its head at once and is
  not cut. Health routes are exempt.
- On a timeout the handler's future is dropped. A transaction it held is rolled back: diesel-async marks the
  connection broken and deadpool discards it on return (DEP-13); `statement_timeout` ends a statement still running
  on the server. A write that committed before the deadline stays; the 504 means "outcome unknown", as live's.
- S3: `TimeoutConfig` with attempt timeout `AGENTD_S3_ATTEMPT_TIMEOUT_MS` (30,000) and operation timeout
  `AGENTD_S3_OPERATION_TIMEOUT_MS` (50,000, below the 60 s request deadline), named like `AGENTD_SECRETS_*`; connect
  3 s as a constant.

### Rate limits (RL1)

- After auth, the edge matches the method and route template to a bucket (L2, exact templates, no prefixes):

| Bucket | Limit a minute | Routes |
|---|---|---|
| create | 300 | `POST /v1/agents`, `POST /v1/sessions`, `POST /v1/environments` |
| read | 1,200 | every `GET` under `/v1/agents`, `/v1/sessions`, `/v1/vaults`, `/v1/deployments`, `/v1/deployment_runs` |
| memory stores | 30,000 | `GET /v1/memory_stores` |
| files | 500 | every route under `/v1/files` |
| none | | everything else, including `POST /v1/sessions/{id}/events`, environments reads, skills, memories |

- One Valkey Lua script per request on key `agentd:rate:{org}:{bucket}`: `INCR`; on 1 set `PEXPIRE 60000`; return
  the count and `PTTL`. Over the limit: 429 `rate_limit_error`, `retry-after` = the seconds left. Every limited route
  except files sends the three `anthropic-ratelimit-requests-*` headers, `reset` being now plus the window's TTL.
- Valkey slow (over 200 ms, as the budget) or down: admit, count `agentd.http.rate_limit_unchecked`, and log `WARN`
  at most once a minute per replica.
- **Load shedding (D14)**: no fixed in-flight cap. When the pool cannot hand out a connection within
  `AGENTD_DB_POOL_WAIT_MS`, the edge answers 529 `overloaded_error` on every route, as the stream route already does
  (`service/stream.rs:84`). Background work (the loop, the webhook claim loop, feed handlers, the sweeper) never answers 529; it
  retries as its own path does.

### Key cache (K1)

- A per-replica map from key digest to `AuthContext` and key id, 60 s TTL, at most 10,000 entries (expired entries
  are cleared when it is full; if still full, the lookup is not cached). Failed lookups are not cached.
- Revocation comes from the change feed: `api_keys` is already on Debezium's feed (R7). A handler for `api_keys`
  updates that set `revoked_at` publishes the key id on Valkey `agentd:keys`; every replica drops it. This covers the
  API, `agentd keys revoke` (R8) and a workspace's keys alike, with no Valkey connection in the CLI. If the message is
  lost, the TTL bounds acceptance to 60 s (D8).
- Environment keys and admin keys are not cached (F-4).

### Event log (E1 to E4)

`processed_at` is always written as `%Y-%m-%dT%H:%M:%S%.6fZ` (`shared/src/time.rs:40`, DEP-14), so its text sorts
as its time. V26, `create index concurrently`, one file:

| Index | Columns | Predicate | Read |
|---|---|---|---|
| `session_events_processed_asc_idx` | `(workspace_id, session_id, (doc->>'processed_at') asc, seq asc)` | `doc->>'processed_at' is not null` | list asc |
| `session_events_processed_desc_idx` | `(workspace_id, session_id, (doc->>'processed_at') desc, seq desc)` | same | list desc |
| `session_events_unprocessed_idx` | `(workspace_id, session_id, seq)` | `doc->>'processed_at' is null` | queued read, list tail, `head` |
| `session_events_tool_use_idx` | `(workspace_id, session_id, (doc->>'tool_use_id'))` | `doc ? 'tool_use_id'` | answers |
| `session_events_custom_tool_use_idx` | `(workspace_id, session_id, (doc->>'custom_tool_use_id'))` | `doc ? 'custom_tool_use_id'` | answers |
| `session_events_idle_idx` | `(workspace_id, session_id, seq)` | `doc->>'type' = 'session.status_idle'` | idle-send guard |

(Six indexes; V26 holds all six.) Each query repeats its index's predicate word for word, since the planner uses a
partial index only when the query's `WHERE` implies the predicate (DEP-15).

- **Answers**: one statement, three arms joined with `UNION` (not `UNION ALL`: a row matched by two arms comes back
  once, as with the `OR`): `event_id = ANY($3)` on the primary key; `doc ? 'tool_use_id' AND doc->>'tool_use_id' =
  ANY($3)`; the same for `custom_tool_use_id`.
- **List**: processed rows first, from the asc or desc index, keyset `(processed_at text, seq)` from the token; when
  a page runs past them, the unprocessed tail from the unprocessed index by `seq` (nulls last in both orders, as
  today). The token records which phase it is in. Type and thread filters stay as filters on rows read in index
  order, so a selective filter reads more rows than the page holds.
- **Idle guard**: `SELECT doc ... WHERE <ws, session> AND doc->>'type' = 'session.status_idle' ORDER BY seq DESC
  LIMIT 1`. An idle event is processed when it is appended, so its `seq` order is its `processed_at` order.
- **Head**: `max(seq)` on the primary key, and the queued read on the unprocessed index.

### Outbound calls (O1, O2; D16, D17)

Today model calls retry with full jitter; AWS calls use the SDK's standard retries; MCP and token refresh retry at the
next turn after a `session.error` marked `retrying` (MC-14, live's behaviour); nothing has a breaker.

- **Model breaker**, per provider per replica, in memory (design skill exception). 5 retryable failures in a row
  (408, 409, 429, 5xx, transport), across all steps on the replica, open it for 30 s, or until the provider's
  `retry-after` if later. While open, a step's retry does not call the provider and does not fail: it waits until the
  breaker lets calls through again, and that wait counts as backoff, spends none of the step's 5 tries, and stays
  inside the step's 600 s deadline. After the open time one call goes through; a success closes the breaker, a
  failure opens it again. Steps keep live's end: only a step whose tries or deadline run out ends its turn with
  `retries_exhausted` (`worker/step.rs:146-149`), as today. A breaker that failed fast would end turns sooner, so it
  does not.
- No in-call retry for MCP or token refresh: live reports the failure as an event and retries at the next turn,
  and an in-call retry would hide that event.

### Signals (G1)

- Channels `agentd:session:{workspace}:{session}` and `agentd:steps`. A watch or a stream subscribes to its session
  and unsubscribes when done; idle claimers listen to `agentd:steps` only.
- Standalone Valkey: one pub/sub connection per replica. Cluster: `SSUBSCRIBE` and `SPUBLISH`; the redis-rs cluster
  client (RESP3 with a push sender, as `valkey.rs:94-95` already sets) holds a connection to each shard that owns a
  subscribed slot (DEP-16).
- The `Signals` port gains `subscribe_session` and `subscribe_steps`; `LocalSignals` follows.

### Files (F1)

- `BlobStore` gains `put_stream(key, stream, len: Option<u64>)` and `get_stream(key) -> (stream, len)`. The S3 adapter
  buffers up to 8 MiB: if the stream ends first, one `put_object`; otherwise multipart in 8 MiB parts (5 MiB minimum,
  10,000 parts, so up to 80 GB: DEP-17), aborted on error. Docs name an `AbortIncompleteMultipartUpload` lifecycle
  rule (1 day) for uploads a crash left.
- Upload reads the multipart field chunk by chunk, counting bytes against 500 MB; download streams the body with
  `content-length`.

### MCP (M1, M2)

- `Guard` keeps clients in a per-replica map keyed by host, pinned addresses, timeout and user agent, 5 min TTL (then
  DNS and the address check run again), at most 1,024 entries (built like the key cache). `Guard::with_user_agent`
  lets the webhook sender keep `Claude-Platform`; MCP keeps `Claude-User`. This lands in PR 1, before the sender uses
  it.
- Replies are read chunk by chunk up to 16 MiB (a constant); over it the call fails as an unreachable server does.

### Lists (P2)

Keyset, token still `page_<base64>`, one statement per page each:

| List | Sort key | Statement |
|---|---|---|
| sessions | `(created_at, session_id)`, both orders, `prev_page` | `SELECT ... WHERE workspace_id = $1 AND <filters> AND (created_at, session_id) < ($2, $3) ORDER BY created_at DESC, session_id DESC LIMIT $4` (asc mirrors it) |
| work items | `(created_at, work_id)` | same shape |
| agents | `(created_at, agent_id)` | same shape |
| memory versions | `seq` | `... AND seq < $2 ORDER BY seq DESC LIMIT $3` on `memory_versions_store_seq_idx` |
| deployment runs | `(created_at, run_id)` | same shape, paging moved from the route into SQL |
| agent versions | `version` | `... AND version < $2 ORDER BY version DESC LIMIT $3` |
| memories, depth 0 | `(path, memory_id)` | `... AND deleted = false AND (path, memory_id) > ($2, $3) ORDER BY path, memory_id LIMIT $4` |

- Memories at depth above 0 keep grouping in memory, bounded by 10,000 memories per store (L4).
- Kept on OFFSET: skills, skill versions, environments, vaults, memory stores, deployments (small per workspace).
  Kept in memory: threads, resources, vault credentials (20).
- The build confirms each sort key against the table's primary key and existing indexes; a list without a matching
  index gets one in V27 only with approval (`files` already does: below).

### Retention (X1 to X4)

- **One sweeper**, every 5 min on every replica. Each batch is one transaction that starts with
  `pg_try_advisory_xact_lock(hashtext('agentd.sweeps'))`; if another replica holds it, this replica skips the round.
  The lock goes with the transaction, so a pooled connection never keeps it. A round repeats batches, table by table,
  until a batch returns fewer than 1,000 rows.
- Batches of 1,000, two statements each (select keys, then `DELETE ... USING unnest`); blobs before rows, so a crash
  leaves a row past retention that the next round deletes, never an orphan blob:

| Table | Select | Retention |
|---|---|---|
| deliveries delivered | `SELECT webhook_id, endpoint_key FROM webhook_deliveries WHERE delivered_at IS NOT NULL AND delivered_at < now() - interval '24 hours' LIMIT 1000` | 24 h |
| deliveries dropped | the same on `dropped_at`, 30 days | 30 days |
| memory versions | `SELECT v.workspace_id, v.store_id, v.version_id FROM memory_versions v LEFT JOIN memories m ON m.workspace_id = v.workspace_id AND m.store_id = v.store_id AND m.memory_id = v.memory_id AND m.head_version_id = v.version_id AND NOT m.deleted WHERE v.created_at < now() - interval '30 days' AND m.memory_id IS NULL LIMIT 1000` (a join on the primary key, no subquery); blobs 16 at a time as `memory/stores.rs:109` | 30 days, the current version of a live memory kept |
| files | `SELECT workspace_id, file_id FROM files WHERE doc ? 'expires_at' AND doc->>'expires_at' < $1 LIMIT 1000`, `$1` the cutoff in the fixed format (DEP-14) | 30 days after `expires_at` |

- Event retention keeps its setting, off by default (L6), and deletes at most 10,000 rows per statement:
  `SELECT seq ... ORDER BY seq OFFSET 10000 LIMIT 1` gives the bound, then `DELETE ... AND seq < $3`.
- Expired files: listed and their metadata readable for 30 days, content 404 (L5).
- `fire_claims` keeps the scheduler's own hourly sweep (DS-15, the scheduler's scope; one row per fire, far fewer than
  deliveries). Follow-up F-6 moves it into this sweeper, so there is one way.
- V27, `create index concurrently`, one file:
  - `webhook_deliveries_delivered_idx on webhook_deliveries (delivered_at) where delivered_at is not null`
  - `webhook_deliveries_dropped_idx on webhook_deliveries (dropped_at) where dropped_at is not null`
  - `memory_versions_created_idx on memory_versions (created_at)`
  - `files_expires_idx on files ((doc->>'expires_at')) where doc ? 'expires_at'`
  - `files_ws_created_idx on files (workspace_id, created_at desc, file_id desc)` (the files list has none)


## Migrations

They follow the durable scheduler's V22 and V23; each table change starts with `set local lock_timeout = '5s';` (V16, V20, V21 do); index
files use `concurrently` as V10 to V12. No guards or row checks (`project-agentd-not-in-production`), no TRUNCATE.

- **V24** (PR 1 commit 2):
  `create table webhook_endpoint_states (endpoint_key text primary key, org_id text not null, failing_since
  timestamptz, disabled_at timestamptz, disabled_reason text, config_digest text not null);`
  `alter table webhook_deliveries add column last_status smallint, add column last_error text;`
  - `endpoint_key` alone is the key: endpoint ids are unique across orgs (`endpoints.rs` `by_key`). `org_id` stays for
    the operator command and logs. `disabled_reason` is text with no `CHECK IN` (rule 20); a Rust enum
    (`redirect`, `private_address`, `failing`) maps it. The table is not on the Debezium feed.
- **V25** (PR 1 commit 5): `alter table loop_steps add column failures bigint not null default 0;`
- **V26** (PR 2 commit 1): the six event log indexes.
- **V27** (PR 2 commit 5): the five retention and list indexes.


## Rollout

agentd is not in production (D12), so no rows need carrying over.

1. Before raising the pool to 16, operators set Postgres `max_connections` to at least replicas × 32 + 20, or put a
   pooler in front (`docs/operations.md`).
2. Flyway runs V24 and V25 before code PR 1's binary starts, and V26 and V27 before code PR 2's. All four only add a
   table, columns or indexes, so the binary before them keeps working on the new schema and a rollback is a revert
   of the binary.
3. During a rolling deploy of code PR 1, an old instance still POSTs first attempts from the change feed and holds
   retries in memory, while a new instance claims the same rows from the table. In that overlap a delivery can go
   out twice with the same `webhook-id`, which receivers already handle (at-least-once). An old instance's
   in-memory retries end when it stops; their rows are still undelivered, so a new instance sends them.
4. Before code PR 2's streamed uploads, the bucket gets an `AbortIncompleteMultipartUpload` lifecycle rule
   (`docs/operations.md`).

## Gaps on main

What the design fixes, as found on `main`. The Plan column names the commit.


| # | Finding (on main unless marked) | Where | Plan |
|---|---|---|---|
| W1 | First webhook attempt inline on the feed, 10 s timeout; one slow endpoint stalls the partition | `webhooks/src/sender.rs:24,137-149` | PR 1 commit 2 |
| W2 | Retries spawned before taking one of 256 permits; permits held through 31 s sleeps | `sender.rs:26,111,152-175` | PR 1 commit 2 |
| W3 | Retries in memory, lost on restart; `next_attempt_at`, `claimed_until`, the due index unused | `sender.rs:5-7`, `V9:106-125` | PR 1 commit 2 |
| W4 | The sender skips the egress guard; a private address is sent to | `sender.rs:105-110` | PR 1 commit 2 |
| R1 | A failed mark makes the feed redo the first POST | `sender.rs:137-149` | PR 1 commit 2 |
| V1 | Valkey ordered lease: GET then PEXPIRE, result ignored, never rechecked in a 100-entry batch | `signals-valkey/src/changes.rs:126-158` | PR 1 commit 3 |
| V2 | Unordered entries one after another; past 60 s another replica redoes them | `changes.rs:117-122,175-187` | PR 1 commit 3 |
| SD1 | SIGTERM: the loop keeps claiming; steps dropped with leases held (8 to 12 min stall); HTTP drain 10 s | `main.rs:83-107`, `backends.rs:170`, `worker/mod.rs:131-157` | PR 1 commit 4 |
| S1 | A failed step retries every 5 s forever; `attempts` is a lifetime fence | `worker/step.rs:31-38`, `loop_steps/claim.rs:59` | PR 1 commit 5 |
| BU1 | Over-budget steps all retry at :00 of the next minute | `signals-valkey/src/budget.rs:44-72` | PR 1 commit 6 |
| T1 | No `statement_timeout` or `idle_in_transaction_session_timeout` | `store-postgres/src/pool.rs:52-84` | PR 1 commit 7 |
| T2 | No request deadline; no `timeout_error` type | `api/src/edge/mod.rs:40-48`, `shared/src/error.rs` | PR 1 commit 7 |
| T3 | S3 client has no timeouts | `blobs-s3/src/lib.rs:74-81` | PR 1 commit 7 |
| RL1 | No rate limit; `rate_limited()` has no caller | `edge/mod.rs`, `error.rs:148` | PR 1 commit 8 |
| K1 | One Postgres read per request for the key | `api-keys/src/service.rs:112-118` | PR 1 commit 9 |
| P1 | Pool default 10; spec S-8 says 16 | `api/src/config/store.rs:55` | PR 1 commit 12 |
| M1 | A new `reqwest::Client` per MCP open; the webhook sender would pay the same through `Guard` | `shared/src/net.rs:60-69` | PR 1 commit 1 |
| E1 | Answering a tool call scans the session on a JSON `OR` | `session_events.rs:78-80` | PR 2 commit 1 |
| E2 | Queued-events read and stream-start `head` scan the session | `session_events/reads.rs:16-17,68-69,165-167` | PR 2 commit 1 |
| E3 | Events list sorts on `processed_at` cast per row; `OFFSET` paging | `reads.rs:93-108` | PR 2 commit 1 |
| E4 | The idle-send guard runs that list on every send | `sessions/src/service/waiting.rs:29-37` | PR 2 commit 1 |
| G1 | One global signal channel; every claimer and watch wakes on any session | `signals-valkey/src/lib.rs:28-93` | PR 2 commit 2 |
| F1 | Upload buffers up to 500 MB; one `put_object`; download collects the object | `files/src/routes.rs:68,152-164`, `blobs-s3/src/lib.rs:119-142` | PR 2 commit 3 |
| M2 | MCP reply read whole, no cap | `mcp/client.rs:126` | PR 1 commit 11 |
| O1 | No breaker on model calls: under a provider's 429 or 529 every step still makes its 5 tries | `engine/src/model/retry.rs:55-80` | PR 1 commit 10 |
| O2 | MCP `initialize`, `tools/list` and token refresh are tried once per turn; a failure is a `session.error` marked `retrying` and the next turn tries again, as live (MC-14, B27) | `engine/src/mcp/errors.rs:17-19` | kept, matches live |
| P2 | OFFSET paging in 11 lists; memory, deployment runs, agent versions page in memory | `shared/src/page.rs:155-163` and below | PR 2 commit 4 |
| X1 | `webhook_deliveries` never deleted from | `webhook_deliveries.rs` | PR 2 commit 5 |
| X2 | `memory_versions` never deleted from | `memory/versions.rs` | PR 2 commit 5 |
| X3 | Expired files never swept, hidden at once | `files.rs:174-176`, `V1:71-73` | PR 2 commit 5 |
| X4 | Event retention deletes up to 500 sessions per statement, rows unbounded | `sessions/src/retention.rs:14-162` | PR 2 commit 5 |
| R5 | `history.*` never deleted from | V17 | by design (paid history) |
| R6 | Skills and memory head deletes remove blobs one at a time | `skills.rs:182`, `memory/heads.rs:109` | follow-up F-3 |
| R7 | `api_keys`, `environment_keys`, `admin_keys` and `webhook_deliveries` are on the Debezium feed | `docker/debezium/application.properties:38` | used by SH-17; sized below |
| R8 | `agentd keys revoke` runs in a process with Postgres only | `api/src/cli.rs:40-45` | covered by SH-17 (feed) |
| R9 | `webhook_deliveries`, `loop_steps` are not in Diesel's `schema.rs`; their SQL is `sql_query` | `store-postgres/src/schema.rs` | `schema.rs` untouched |

Left for follow-ups F-4 and F-8 to F-11 in [decisions.md](decisions.md): environment and admin key caches; the
busy-workspace claim and its index (`loop_steps/claim.rs:25-56`); the `set_config` round trip per checkout
(`checkout.rs:67-81`); the credential metadata GET reading the secret, and vault delete purging one by one; rubric
files per step.

**File sizes** (production lines without imports; the limit is 200 and a file over it never grows):

| File | Lines | Plan |
|---|---|---|
| `deployments/src/types.rs` | 266 | not touched |
| `engine/src/mcp/client.rs` | 199 | the body read moves to `mcp/reply.rs` (PR 1 commit 11); net down |
| `files/src/service.rs` | 198 | new code in `files/src/upload.rs`; visibility moves to `files/src/expired.rs`; net down |
| `shared/src/error.rs` | 196 | the status and type table moves to `error/http.rs` (new) before `Timeout` is added; net down |
| `store-postgres/src/entities/agents.rs` | 195 | the list moves to `agents/list.rs`; net down |
| `store-postgres/src/entities/session_events.rs` | 193 | the answer lookup moves to `session_events/answers.rs`; net down |
| `store-postgres/src/schema.rs` | 190 | not touched (R9) |
| `signals-valkey/src/changes.rs` | 187 | the unordered pass moves to `changes/unordered.rs`; net down |
| `session_events/reads.rs` | 184 | the page and head reads move to `reads/page.rs`; net down |
| `api/src/cli/command.rs` | 184 | one arm (+3) |
| `shared/src/net.rs` | 180 | the client cache in `net/clients.rs`; +4 |
| `webhooks/src/sender.rs` | 177 | split into `sender/*`; net down |
| `sessions/src/retention.rs`, `service/stream.rs` | 177 each | bounded delete +6; stream: the tail map moves to `stream/tails.rs`, net down |
| `loop_steps/held.rs` | 174 | `defer` and `defer_failed` move to `held/defer.rs`; net down |
| `api/src/config.rs` | about 175 | one field per new config module; +3 |

The build rechecks every count on current `main`; a file that crosses 200 gets its split commit first.

## Plan of PRs

### Code PR 1: correctness under load (one session; about 70 files; SH-1 to SH-21, SH-25, SH-26, SH-29 to SH-36, SH-42 to SH-44, SH-47, SH-48, SH-52, SH-53)

1. **Client cache** (SH-20, SH-25). `shared/src/net/clients.rs` (new, the map); `net.rs` uses it and gains
   `with_user_agent`. Tests: "two calls to one host reuse a client", "after 5 min the address is checked again".
2. **Webhook sender** (SH-1 to SH-4, SH-29 to SH-34, SH-36, SH-43). `V24__webhook_endpoint_states.sql`.
   `webhooks/src/sender.rs` (177) becomes `sender.rs` (feed handler: publishes the wake), `sender/claim.rs` (wake
   subscription, 1 s backstop, 50 ms fold, queue store, senders), `sender/senders.rs` (free senders, breaker state per
   endpoint), `sender/breaker.rs`, `sender/send.rs` (guard client, sign, POST, outcome: queue store, health, metrics),
   `sender/health.rs` (endpoint state store, the 10 s read); `shared/src/ports/broadcast.rs` (new port: `publish(channel, payload)`,
   `subscribe(channel)`) and `signals-valkey/src/broadcast.rs` (new, its Valkey adapter, built on the `subscribe`
   helper in `valkey.rs`; the wake is channel `agentd:webhooks`). Each type at most 5 dependencies. Store: `webhook_deliveries.rs` becomes a
   folder:
   - `insert.rs`: today's inserts.
   - `queue.rs`, claim, one transaction: `SELECT webhook_id, endpoint_key FROM webhook_deliveries WHERE delivered_at IS
     NULL AND dropped_at IS NULL AND next_attempt_at <= now() AND (claimed_until IS NULL OR claimed_until < now()) AND
     endpoint_key <> ALL($1) ORDER BY next_attempt_at LIMIT $2 FOR NO KEY UPDATE SKIP LOCKED`, then `UPDATE
     webhook_deliveries AS d SET claimed_until = now() + interval '60 seconds', attempt = d.attempt + 1 FROM
     unnest($3::text[], $4::text[]) AS v(webhook_id, endpoint_key) WHERE d.webhook_id = v.webhook_id AND
     d.endpoint_key = v.endpoint_key AND d.delivered_at IS NULL AND d.dropped_at IS NULL AND (d.claimed_until IS NULL
     OR d.claimed_until < now()) RETURNING d.webhook_id, d.endpoint_key, d.attempt, d.endpoint_url, d.body`. The
     sender sends only the returned rows.
   - `queue.rs`, release unsent: `UPDATE webhook_deliveries SET claimed_until = NULL, attempt = attempt - 1 WHERE
     webhook_id = $1 AND endpoint_key = $2 AND attempt = $3`.
   - `queue.rs`, outcome: `UPDATE webhook_deliveries SET {delivered_at = now() | dropped_at = now() | next_attempt_at
     = now() + make_interval(secs => $4)}, claimed_until = NULL, last_status = $5, last_error = $6 WHERE webhook_id
     = $1 AND endpoint_key = $2 AND attempt = $3`.
   - `queue.rs`, oldest due: `SELECT min(next_attempt_at) FROM webhook_deliveries WHERE delivered_at IS NULL AND
     dropped_at IS NULL`.
   - `endpoints.rs`, failure: `INSERT INTO webhook_endpoint_states (endpoint_key, org_id, failing_since,
     config_digest) VALUES ($1, $2, now(), $3) ON CONFLICT (endpoint_key) DO UPDATE SET failing_since =
     coalesce(webhook_endpoint_states.failing_since, now()), disabled_at = CASE WHEN
     webhook_endpoint_states.failing_since < now() - interval '120 hours' THEN coalesce(webhook_endpoint_states
     .disabled_at, now()) ELSE webhook_endpoint_states.disabled_at END, disabled_reason = CASE WHEN
     webhook_endpoint_states.failing_since < now() - interval '120 hours' THEN coalesce(webhook_endpoint_states
     .disabled_reason, 'failing') ELSE webhook_endpoint_states.disabled_reason END RETURNING disabled_at IS NOT NULL
     AS disabled`. One statement per failed attempt.
   - `endpoints.rs`, 3xx or private address: `INSERT INTO webhook_endpoint_states (endpoint_key, org_id,
     config_digest, disabled_at, disabled_reason) VALUES ($1, $2, $3, now(), $4) ON CONFLICT (endpoint_key) DO
     UPDATE SET disabled_at = coalesce(webhook_endpoint_states.disabled_at, now()), disabled_reason =
     coalesce(webhook_endpoint_states.disabled_reason, excluded.disabled_reason)`.
   - `endpoints.rs`, 2xx to a failing endpoint: `UPDATE webhook_endpoint_states SET failing_since = NULL WHERE
     endpoint_key = $1 AND failing_since IS NOT NULL`.
   - `endpoints.rs`, every 10 s: `SELECT endpoint_key, failing_since IS NOT NULL AS failing, disabled_at IS NOT NULL
     AS disabled FROM webhook_endpoint_states WHERE failing_since IS NOT NULL OR disabled_at IS NOT NULL` (one row per
     endpoint; the table is small).
   - `endpoints.rs`, boot: `INSERT INTO webhook_endpoint_states (endpoint_key, org_id, config_digest) SELECT * FROM
     unnest($1::text[], $2::text[], $3::text[]) ON CONFLICT (endpoint_key) DO UPDATE SET org_id = excluded.org_id,
     config_digest = excluded.config_digest, disabled_at = CASE WHEN webhook_endpoint_states.config_digest =
     excluded.config_digest THEN webhook_endpoint_states.disabled_at END, disabled_reason = CASE WHEN ... THEN
     webhook_endpoint_states.disabled_reason END, failing_since = CASE WHEN ... THEN
     webhook_endpoint_states.failing_since END`. `config_digest` is written on every insert, so it is never null.
   Tests (`api/tests/webhooks.rs`): "a new delivery is sent within 100 ms of its insert"; "three instances share 3,000
   due deliveries, each sends some, none twice"; "a dead endpoint does not delay another endpoint's delivery"; "a
   delivery the feed hands over twice is sent once"; "a delivery left by a stopped instance is sent by another";
   "the first retry waits 15 to 30 s, the second 30 to 60 s, and the delivery drops after 3 attempts"; "two replicas send a delivery once";
   "20 claimers on separate connections, started together on 1,000 due rows, 50 rounds: no row is claimed twice and
   every row is claimed once"; "a row claimed by another instance between the select and the update is not returned";
   "a live claim is not taken over, an expired one is"; "a row with under 15 s of claim left is released unsent, its
   attempt unchanged"; "a late outcome from an expired claim changes nothing"; "an open breaker spends
   no attempt"; "a 3xx disables at once"; "a private address is not sent to and disables"; "failing for 120 h disables,
   a 2xx in between restarts the clock"; "a disabled endpoint's new delivery is dropped unsent"; "a changed URL
   re-enables at boot, an unchanged config keeps it disabled"; unit tests for the wake fold, the senders and the breaker.
3. **Valkey lease** (SH-5, SH-6). `signals-valkey/src/changes/lease.rs` (new: script, ticker); `changes.rs` (187)
   moves the unordered pass to `changes/unordered.rs`. Tests: "a lease taken over mid-batch stops the old holder",
   "an entry longer than 30 s keeps its lease", "a slow unordered batch is handled once".
4. **Shutdown** (SH-7, SH-8, SH-42). `api/src/main.rs` (131) builds one deadline and a `watch` signal, HTTP drain to
   the grace; `engine/src/worker/mod.rs` (160) stops claiming and keeps steps in a `JoinSet`;
   `engine/src/worker/drain.rs` (new). `sessions/src/steps.rs` (170) gains `release_all`; `loop_steps/release.rs`
   (new): `UPDATE loop_steps SET lease_owner = NULL, lease_until = NULL, due_at = now() FROM unnest($1::text[],
   $2::text[], $3::bigint[]) AS v(workspace_id, session_id, attempts) WHERE loop_steps.workspace_id = v.workspace_id
   AND loop_steps.session_id = v.session_id AND loop_steps.attempts = v.attempts AND loop_steps.lease_owner = $4`.
   `config/role.rs` the grace. The webhook claim loop takes the same signal. Tests: "a step running at
   SIGTERM is due at once on another worker", "SIGTERM with a stuck send, a long step and an open request ends within
   25 s".
5. **Step backoff** (SH-9). `V25__loop_steps_failures.sql`; `loop_steps/held.rs` (174) moves `defer` and
   `defer_failed` to `held/defer.rs`: `UPDATE loop_steps SET failures = failures + 1, lease_owner = NULL, lease_until
   = NULL, due_at = now() + random() * least(300, 5 * 2 ^ failures) * interval '1 second' WHERE {HELD}`; the commit's
   existing `UPDATE` adds `failures = 0`. `worker/step.rs` calls it. Tests: `pg_loop_steps` "the first wait is at most
   5 s, the waits grow, cap at 5 min, and a commit resets them".
6. **Budget jitter** (SH-53). `signals-valkey/src/budget.rs` (80). Unit test for the spread.
7. **Timeouts** (SH-10 to SH-12, SH-21). `store-postgres/src/pool.rs` (90): the two `SET`s in `custom_setup`;
   `api/src/config/store.rs` (60) the statement timeout; `api/src/edge/deadline.rs` (new); `edge/mod.rs` (23) one
   layer; `shared/src/module.rs` (73) the per-module timeout; `files/src/lib.rs`; `shared/src/error.rs` (196) moves its
   status and type table to `error/http.rs` (new), then gains `Timeout`; `blobs-s3/src/lib.rs` (148) `TimeoutConfig`;
   `api/src/config/blobs.rs` (new) the S3 settings. Tests: `pg_pool` "a statement over the timeout fails";
   `api/tests/edge` "a handler past the deadline answers 504 timeout_error and its transaction is rolled back", "a
   stream is not cut", "health answers during a slow request".
8. **Rate limits** (SH-13 to SH-15). `shared/src/ports/rate.rs` (new port: `check(org, bucket) -> Verdict`);
   `signals-valkey/src/rate.rs` (new, the window script, one `EVALSHA` per request); `api/src/edge/rate.rs` (new: the
   route table, 429, headers); `edge/mod.rs`. Tests: "the 301st create in a window answers 429 with retry-after",
   "reads share one bucket across agents and sessions", "memory stores have their own bucket", "events send and
   skills are not limited", "headers match live's on a read", "Valkey down admits and counts".
9. **Key cache** (SH-16 to SH-18). `api-keys/src/cache.rs` (new) wrapping `ApiKeysService`; `api-keys/src/revoked.rs`
   (new, the `api_keys` feed handler); the broadcast port from commit 2 on channel `agentd:keys`. Tests: "a cached key
   reads Postgres once", "a key revoked through `agentd keys revoke` is refused on another replica within 1 s", "a key
   revoked with no message is refused after the TTL".
10. **Model breaker** (SH-47, SH-48). `engine/src/model/breaker.rs` (new: per provider, open, half-open, close);
    `model/retry.rs` (87) asks it before each try and folds its wait into the backoff. Tests: "after 5 failures the
    provider gets no call for 30 s and steps wait, not fail", "a retry-after longer than 30 s holds the breaker open",
    "one success closes it", "a step whose deadline passes while open ends with retries_exhausted as today".
11. **MCP reply cap** (SH-26). `engine/src/mcp/reply.rs` (new, the capped read); `mcp/client.rs` (199) calls it, net
    down. Test: "a reply over 16 MiB fails the call".
12. **Pool and wiring** (SH-19, SH-35, SH-44): `config/store.rs` default 16; `api/src/cli/webhooks.rs`
    (new, `agentd webhooks enable`: `UPDATE webhook_endpoint_states SET disabled_at = NULL, disabled_reason = NULL,
    failing_since = NULL WHERE endpoint_key = $1 AND org_id = $2`) and one arm in
    `cli/command.rs`; `api/src/backends.rs` (webhook claim loop and wake, health read, rate limiter, key cache and its feed handler, the
    shutdown deadline); `config.rs`; `docs/configuration.md` (every new setting), `docs/operations.md` (shutdown,
    deadlines, re-enable, the S3 lifecycle rule, `max_connections`), `docs/observability.md` (the metrics table).
    Tests: config default; "enable re-sends new deliveries".
13. **Webhook load test** (SH-52). `api/tests/load/webhooks.rs` (new, `#[ignore]`,
    run by hand) and fake receivers: fast (100 ms), slow (5 s), dead; many organizations plus one noisy one. Runs: 1,
    2 and 3 instances, on Kafka and on Valkey Streams. Measures delivered webhooks a second per instance count, delay
    from insert to first attempt (p50, p99), claims a second against Postgres, and that no delivery is sent twice. It
    also records how far the noisy organization delays the others, which is the input F-7 needs. The results table
    goes on the PR.

### Code PR 2: speed and growth (one session, stacked on PR 1; about 45 files; SH-22 to SH-24, SH-27, SH-28, SH-37 to SH-41, SH-45, SH-46)

1. **Event log** (SH-22, SH-23). `V26__session_events_indexes.sql`; `session_events.rs` (193) moves the answer lookup
   to `session_events/answers.rs` (new, the `UNION`); `reads.rs` (184) moves `events_page` and `head` to
   `reads/page.rs` (new): list one or two statements per page (processed, then the tail), head two (`max(seq)`, the
   queued read); `waiting.rs` calls `last_idle`. `shared/src/page/keyset.rs` (new, the token). Tests: `pg_event_log`
   "the list pages without repeats or gaps while events arrive, both orders", and one "EXPLAIN uses the index" per
   read on a 50,000-event session.
2. **Signals** (SH-24). `sessions/src/signals.rs` (64) the port; `signals-valkey/src/lib.rs` (97) and
   `signals-valkey/src/channels.rs` (new); `worker/watch.rs`, `worker/mod.rs`, `service/stream.rs` (177; the tail map
   to `stream/tails.rs`); the publish sites. Tests: "a watch wakes only for its session", "a claimer wakes when a step
   is due"; the cluster case runs in the existing Valkey cluster test, the CI job is not touched.
3. **Files** (SH-27). `shared/src/ports/blobs.rs` (44); `blobs-s3/src/multipart.rs` (new); `blobs-s3/src/lib.rs`;
   `api/src/telemetry/ports.rs` (111) decorator; `files/src/routes.rs` (160); `files/src/upload.rs` (new);
   `files/src/store.rs` (116) `insert_streamed`; `store-postgres/.../files/stream.rs` (new: one `INSERT` of the row
   after the blob is up, as today's insert). Tests: "a 20 MiB upload goes up in parts and downloads byte for byte",
   "an upload over 500 MB is 413 with no object left", a memory bound check on a 100 MiB upload.
4. **Lists** (SH-28). `store-postgres/.../sessions/list.rs` (new; the list function moves out of `sessions.rs`); `work_items/reads.rs` (79); `agents/list.rs` (new);
   `memory/versions.rs` (171); deployment runs and agent versions: new `deployment_runs/list.rs` and
   `agent_versions/list.rs` in the store, and the routes call them (`deployments/src/types.rs` untouched);
   `memory/memories.rs` (120), `memory_reads.rs` (169). Tests per list: "pages cover every row once while rows are
   added"; sessions "prev_page returns the previous page".
5. **Retention** (SH-37 to SH-41, SH-45, SH-46). `V27__retention_indexes.sql`; `api/src/sweeps.rs` (new: lock, loop, stop);
   `store-postgres/.../sweeps/{deliveries,memory_versions,files}.rs` (new); `files/src/expired.rs` (new) and
   `files.rs` for expired-file visibility (net kept); `retention.rs` (177) bounded deletes. Tests per sweep: "rows
   past the window go; newer rows and a live memory's current version stay"; "an expired file is listed, its content
   404, gone after 30 days"; "two replicas never sweep at once".
6. **Wiring and docs**: `backends.rs`, `backends/postgres.rs` (the sweeper), the docs pages.


## Values

Chosen where live is silent.

| Value | Default | Reason |
|---|---|---|
| Senders per instance | 256 | today's cap; a bound on one process |
| Claim fold / backstop | 50 ms / 1 s | a new delivery goes out within about 50 ms; at most 20 claims a second per instance; an idle claim is one empty read of the V9 due index; a fixed poll behind a wake is the Oban (1 s) and Graphile Worker (2 s) default; kept over sleeping until the next due time (D18) |
| Retry backoff | 30 s base, doubled per attempt, equal jitter: 15 to 30 s, then 30 to 60 s | L1 documents jittered exponential backoff in 5 to 120 s (D2) |
| Delivery claim | 60 s | D19; 6 × the 10 s send timeout; a crashed replica's send is picked up within 60 s, inside live's 120 s spacing range |
| Send start margin | 15 s | the 10 s send timeout plus 5 s spare, so a POST always ends before its claim does |
| Breaker | 5 failures, 30 s open | a dead endpoint costs one 10 s timeout per 30 s per replica |
| Auto-disable window | 120 h | Svix's default (DEP-8) |
| Endpoint state read | 10 s | 1 statement a second across 10 replicas |
| Shutdown grace | 25 s | Kubernetes' 30 s default minus 5 s (DEP-9) |
| Step backoff | 5 s doubling to 5 min, full jitter | today's 5 s base; full jitter as in the AWS Architecture Blog, "Exponential Backoff And Jitter" |
| Statement / idle-in-transaction timeout | 30 s / 60 s | the longest statement left is a bounded batch |
| Request deadline | 60 s, files 15 min | 500 MB at about 5 Mbit/s |
| S3 attempt / operation | 30 s / 50 s | operation below the request deadline |
| Key cache | 10,000 entries, 60 s | bounded memory; D8 |
| MCP clients / reply cap | 1,024, 5 min / 16 MiB | 160 × live's 100,000-character threshold |
| Model breaker | 5 failures, 30 s or `retry-after` | the same shape as the webhook breaker |
| Delivered / dropped retention | 24 h / 30 days | D24: 24 h covers a feed replay; sizing below |
| Memory versions kept past 30 days | the current version of each live memory | L4's "recent versions" with no count: the smallest reading that keeps every live memory readable |

Settings (`sre-configuration-design`: one only where a deployment must answer a real question; all `_MS`):

| Setting | Default | The question it answers |
|---|---|---|
| `AGENTD_SHUTDOWN_GRACE_MS` | 25,000 | How long does your platform wait between SIGTERM and kill? |
| `AGENTD_DB_STATEMENT_TIMEOUT_MS` | 30,000 | How long may one statement run on your database? |
| `AGENTD_REQUEST_TIMEOUT_MS` | 60,000 | What is your load balancer's idle timeout? |
| `AGENTD_S3_ATTEMPT_TIMEOUT_MS` | 30,000 | How slow is your object store at its worst? |
| `AGENTD_S3_OPERATION_TIMEOUT_MS` | 50,000 | How long may a blob call take with retries? |

Everything else above is a constant. Rate limits are live's values and are constants.

## Sizing

Design point: 10 replicas; 1,000 deliveries a second cluster-wide at peak (10,000 running sessions, a status change
every 10 s, one endpoint); ceiling checked at 10,000 a second.

- **Sends in flight**: in flight = deliveries a second × the receiver's response time. An instance sends at most 256 /
  latency a second, so 10 instances (2,560 senders) hold the 1,000 a second design point while the endpoint answers
  within about 2.5 s; at 100 ms, 100 are in flight cluster-wide. Past that, rows wait in the table: nothing is lost,
  `oldest_due_age` rises, and each added instance adds 256 senders. A slow 2xx does not open the breaker; a dead
  server holds senders only until its breaker opens (5 failures). Claims: at most 20 a second per instance, 200
  cluster-wide, 2 statements each. The load test checks these numbers with the 100 ms and 5 s receivers.
- **Postgres writes per delivery**: insert, claim, outcome, and the sweep's delete = 4; the failure path adds 1 per
  failed attempt; the 2xx clear only for a failing endpoint. 4,000 writes a second at the design point, 40,000 at the
  ceiling (a larger database). The claim loop: at most 20 transactions a second per instance.
- **Sweep**: at the design point about 300,000 delivered rows fall due per 5-minute round, so a round runs about 300
  batches of 1,000, two statements each: about 2 statements a second on average.
- **Change feed**: `webhook_deliveries` is on the feed (R7), so the claim, the outcome and the sweep's delete add 3
  records per delivery (about 600 B each; the delete is smaller): about 4,000 records, about 2 MB a second at the
  design point. Debezium's `skipped.operations` is per
  connector, not per table, so it is not used; the handler ignores updates and deletes as today. `webhook_endpoint_states` is not
  on the feed.
- **Delivery storage**: a row is about 600 B (a body of about 300 B, `event.rs:108`) plus its indexes. 86 M rows a
  day at the design point: 24 h of delivered rows is about 52 GB; at 1 % dropped, 30 days is about 16 GB. 10 × at
  the ceiling.
- **Event log indexes**: an index entry is about 110 B (two ids, a 27-byte timestamp, a seq); the two processed
  indexes add about 220 B per event, the partial ones only on matching events. A 50,000-event session's hot reads
  touch tens of index entries instead of 50,000 rows.
- **Connections**: 16 per pool × the replica's pools (read and write) × 10 replicas = 320. Postgres'
  `max_connections` default is 100, so docs name the need: `max_connections` at least replicas × 32 + 20, or a
  pooler. Today's 10 already needs 200.
- **Valkey**: one script call per limited request plus pub/sub; at 1,200 reads a minute per org it is far under a
  single node's capacity.

## Out of scope

- The durable scheduler (`.specs/durable-scheduler/`): tick bursts, failed fires, `fire_claims` retention (DS-15).
  The Kafka consumer's long-handler case is DS-13; the webhook handler returns at once (SH-1).
- Scheduled fires and rate limits: a fired session create does not pass the HTTP edge, so it takes no bucket here
  (D15, follow-up F-1).
- The replay and the full load test (the webhook load test is in code PR 1), cloud worker auth, Debezium failover
  (runtime-parity CH-1), stage and prod.

