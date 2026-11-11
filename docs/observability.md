# Observability

agentd emits traces, metrics and logs at every boundary it crosses: each request it serves, each SQL statement
and pooled connection, each blob, secret and sandbox operation, and each scheduled fire. Each signal names its
boundary, its operation and its outcome, so a failure points at the boundary that failed. The code lives in
`crates/api/src/telemetry`.

Traces, metrics and logs go over OTLP to any OpenTelemetry collector when `OTEL_EXPORTER_OTLP_ENDPOINT` is
set (`docs/configuration.md`). Logs always go to stdout too.

## Signals

| Boundary | Where | Span | Metric | Failure log |
|---|---|---|---|---|
| Inbound HTTP, every route and unknown paths | `telemetry::http::observe`, a layer on every route | `GET /v1/agents/{id}` with method, route, status code and request id; W3C `traceparent` is honoured | `http.server.request.duration` (s) by method, route template, status code; `http.server.active_requests` | `error` on 5xx, with the cause |
| Event streams | the same layer, on `text/event-stream` responses | inside the request span | `agentd.sse.active_streams`, held until the client disconnects | |
| Event retention | `sessions::retention`, every 5 minutes on the replica that holds its lock | | `agentd.retention.last_success` (s), when this replica last completed a run, while `AGENTD_EVENT_RETENTION_DAYS` is set | `warn` |
| Postgres statements | `telemetry::db`, diesel `Instrumentation` on every pooled connection | `SELECT postgres` and so on, under the request | `db.client.operation.duration` (s) by operation and outcome | `error`; a unique violation is `debug` (it is a 409) |
| Postgres connections | the pool's setup hook | | a failed `CONNECT` in `db.client.operation.duration`; `db.client.connection.count` (idle, used), `.max`, `.pending_requests` | `error` |
| Blobs, secrets, sandbox | `telemetry::ports::Observed` around each port | `blobs.put`, `secrets.get`, `sandbox.launch` | `agentd.port.operation.duration` (s) by port, operation and outcome | `warn` |
| Scheduled fires | `Observed` around each module's fire handler | `fires.deployment` (the one fire kind today) | the same, port `fires` | `warn` |

Every boundary operation is covered: 49 route registrations plus the unknown-path fallback, every statement
through the pool, all 3 operations of each of the 3 outbound ports, and every fire kind. Inside a request, time
between boundaries is the request span's own time. Not covered: the local scheduler's tick loop and the S3
SDK's internal retries (both log, but emit no metric).

A 5xx response body says only `Internal server error` (or `Overloaded` for a 529), as the live API does. The
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
| `agentd-port-failures` | a blob, secret or sandbox port fails more than 0.05 times per second for 5 minutes | warn |
| `agentd-fire-failures` | any scheduled fire failed in the last 15 minutes | warn |
| `agentd-pool-saturated` | a request waited for a Postgres connection for 5 minutes | warn |
| `agentd-absent` | no agentd telemetry for 5 minutes | page |

The rules are in `docker/observability/grafana/alerts.yaml`. They read the metric names Prometheus gives
OTLP metrics (`http_server_request_duration_seconds` and so on) under `job="agentd"`.
