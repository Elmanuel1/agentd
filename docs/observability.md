# Observability

agentd emits traces, metrics and logs at every boundary it crosses: each request it serves, each SQL statement
and pooled connection, each blob and secret operation, and each scheduled fire. Each signal names its
boundary, its operation and its outcome, so a failure points at the boundary that failed. The code lives in
`crates/api/src/telemetry`.

Traces, metrics and logs go over OTLP to any OpenTelemetry collector when `OTEL_EXPORTER_OTLP_ENDPOINT` is
set (`docs/configuration.md`). Logs always go to stdout too.

## Signals

| Boundary | Where | Span | Metric | Failure log |
|---|---|---|---|---|
| Inbound HTTP, every route and unknown paths | `telemetry::http::observe`, a layer on every route | `GET /v1/agents/{id}` with method, route, status code and request id; W3C `traceparent` is honoured | `http.server.request.duration` (s) by method, route template, status code; `http.server.active_requests` | `error` on 5xx, with the cause |
| Event streams | the same layer, on `text/event-stream` responses | inside the request span | `agentd.sse.active_streams`, held until the client disconnects | |
| The step queue | `telemetry::steps`, a count every 10 s on every replica | | `agentd.loop.steps_due` and `agentd.loop.steps_leased`, the queue's due and leased steps; every replica reads the same table, so take the maximum, not the sum | `warn` |
| The loop | `engine::metrics`, in every step | | `agentd.loop.step_wait` (s), a due step's wait for a worker; `agentd.loop.model_call_duration` (s) by provider and outcome, retries included; `agentd.loop.turns` by stop reason; `agentd.loop.events_read` per step and `agentd.loop.events_appended`; `agentd.loop.compactions` and `agentd.loop.mcp_calls` by outcome; `agentd.loop.step_failures`, steps that failed and wait to run again; `agentd.loop.released`, steps released when the shutdown grace ended; `agentd.model.breaker_opened` by provider | `warn` on a failed step and an opened breaker |
| Webhooks | `telemetry::webhooks`, in the claim loop and every send; the two gauges read every 10 s on every replica | | `agentd.webhooks.deliveries` by `outcome` (`delivered`, `retried`, `dropped`); `agentd.webhooks.claims` by `instance`; `agentd.webhooks.breaker_opened`; `agentd.webhooks.oldest_due_age` (s) and `agentd.webhooks.endpoints_disabled` by `reason`, the same table on every replica, so take the maximum | `info` on a drop, `warn` on a failed write, `error` on a disabled endpoint |
| The edge's limits | `telemetry::edge`, in the rate limit, the request deadline and the key cache | | `agentd.http.rejected` by `reason` (`rate_limited`, `timeout`); `agentd.http.rate_limit_unchecked`; `agentd.keys.cache` by `result` (`hit`, `miss`) | `warn` at most once a minute while rate checks go unanswered |
| The change feed's slot | `telemetry::feed`, a read of the slot every 10 s on every replica | | `agentd.feed.slot.retained_wal` and `agentd.feed.slot.safe_wal` (bytes), `agentd.feed.slot.lost` (0 or 1), `agentd.feed.wal_written` (bytes, a counter); take the maximum across replicas; absent while the slot cannot be read | `warn` |
| Resource history | `history`, in every recorded change | | `agentd.history.apply_delay` (s), how long after its commit a change reached history (buckets 0.1 s to 1 h) | `warn` on a store failure, retried; `error` on a change it cannot record |
| Event retention | `sessions::retention`, every 5 minutes on the replica that holds its lock | | `agentd.retention.last_success` (s), when this replica last completed a run, while `AGENTD_EVENT_RETENTION_DAYS` is set | `warn` |
| Postgres statements | `telemetry::db`, diesel `Instrumentation` on every pooled connection | `SELECT postgres` and so on, under the request | `db.client.operation.duration` (s) by operation and outcome | `error`; a unique violation is `debug` (it is a 409) |
| Postgres connections | the pool's setup hook | | a failed `CONNECT` in `db.client.operation.duration`; `db.client.connection.count` (idle, used), `.max`, `.pending_requests` | `error` |
| Blobs, secrets | `telemetry::ports::Observed` around each port | `blobs.put`, `secrets.get` | `agentd.port.operation.duration` (s) by port, operation and outcome | `warn` |
| Scheduled fires | `Observed` around each module's fire handler | `fires.deployment` (the one fire kind today) | the same, port `fires` | `warn` |
| The scheduler | `telemetry::scheduler`, in the tick and the fire workers; the due count every 30 s on every replica | | `agentd.scheduler.fires` by `outcome` (`enqueued`, `ran`, `retried`, `skipped`, `dropped`); `agentd.scheduler.fire.lag` (s), a run's start minus its due time (buckets 0.5 s to 30 minutes); `agentd.scheduler.fires.due`, the due fires not done, the same table on every replica, so take the maximum | `warn` on a retry or a skip, `error` on a drop |

Every boundary operation is covered: 49 route registrations plus the unknown-path fallback, every statement
through the pool, all 3 operations of each of the 3 outbound ports, and every fire kind. Inside a request, time
between boundaries is the request span's own time. Not covered: the schedule outbox's relay and the S3 SDK's
internal retries (both log, but emit no metric).

A 5xx response body says only `Internal server error` (or `Overloaded` for a 529, `Request timed out` for a 504), as the live API does. The
real cause stays on the server, in the `request failed` log line and the span.

Histograms use the OpenTelemetry HTTP buckets in seconds (5 ms to 10 s).

## Local stack

`docker/observability/compose.yaml` runs Grafana with the OpenTelemetry collector, Prometheus, Tempo and Loki
(`grafana/otel-lgtm`), with the agentd dashboard and alert rules provisioned:

```bash
export GRAFANA_ADMIN_PASSWORD=$(openssl rand -hex 16)
docker compose -f docker/observability/compose.yaml up -d
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 ./target/debug/agentd
docker/observability/verify.sh
```

Grafana is at http://localhost:3000 (`admin` / `GRAFANA_ADMIN_PASSWORD`, which the stack requires; anonymous access is off), dashboard **agentd boundaries**: request rate, 5xx
ratio, p95 and open streams; requests, latency and errors by route; statements, their p95 and failures; the
connection pool; port operations by outcome; error and warning logs; failed traces.

`verify.sh` checks that every metric family reached Prometheus, that Tempo has traces and Loki has logs, and
that the dashboard and alert rules are provisioned.

## Alerts

| Rule | Fires when | Severity |
|---|---|---|
| `agentd-5xx-ratio` | more than 1% of requests answer 5xx for 5 minutes | page |
| `agentd-p95-latency` | p95 above 1 s for 10 minutes | warn |
| `agentd-sql-failures` | more than 0.05 failed statements or connections per second for 5 minutes | page |
| `agentd-port-failures` | a blob or secret port fails more than 0.05 times per second for 5 minutes | warn |
| `agentd-fire-failures` | any scheduled fire failed in the last 15 minutes | warn |
| `agentd-pool-saturated` | a request waited for a Postgres connection for 5 minutes | warn |
| `agentd-absent` | no agentd telemetry for 5 minutes | page |

The rules are in `docker/observability/grafana/alerts.yaml`. They read the metric names Prometheus gives
OTLP metrics (`http_server_request_duration_seconds` and so on) under `job="agentd"`.
