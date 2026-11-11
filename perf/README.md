# Load tests (k6)

`perf/agentd.js` drives agentd the way the official SDKs do (their headers, betas and request shapes), in four
scenarios running together:

| Scenario | What it does | Load (defaults) |
|---|---|---|
| `crud` | agents create and list, sessions create and list, files list, skills list, in rotation | `RATE` = 20 per second |
| `events` | `user.message` events sent to 20 existing sessions | 2 x `RATE` per second |
| `streams` | `STREAMS` concurrent event streams; each opens a stream, sends 5 events, deletes the session and checks every event arrived in order, ending with `session.deleted` | `STREAMS` = 50 |
| `files` | 1 MB uploads, metadata reads, and skill bundle downloads (uploaded files are not downloadable, as live) | `RATE` / 4 per second |

No model is called (the stack lists no workspace in `AGENTD_LOOP_WORKSPACES` and stores no model key) and nothing reaches AWS: the stack is agentd, Postgres 16 and
S3 on LocalStack (MinIO no longer publishes public images).

## Run

```bash
perf/run.sh
```

It builds agentd's image, starts `docker/docker-compose.yml` with `perf/compose.yaml` over it (no host ports), runs
k6 in the same network, and writes `perf/results/summary.json`. `RATE`, `STREAMS` and `DURATION` (default `2m`)
change the load. `perf/run.sh --observe` also sends agentd's telemetry to `docker/observability` (start it
first), and `perf/run.sh --down` removes the stack.

## Thresholds

`perf/thresholds.json`: no more than 1% failed requests, 99% of checks passing, every stream receiving all of its
events, and each endpoint's p95 at most 3 x its baseline p95 (25 ms floor). A stream lasts about 2.8 s by design
(5 events 500 ms apart, then the delete), so its p95 bound is 3.5 s. The bounds hold for the 2-minute default;
a shorter run weighs cold-start requests more, so use `THRESHOLDS=thresholds.smoke.json` for one.

The manual `perf-smoke` workflow runs with `perf/thresholds.smoke.json`, which keeps only the failure-rate
bounds: the latency bounds come from a laptop and a shared runner is not that laptop.

## Baseline

Docker Desktop on Apple silicon (5 CPUs, 10 GB for the VM), defaults (20 crud/s, 40 events/s, 50 streams, 5 file
rounds/s, 2 minutes): 26,629 requests, 0 failed, 11,000 stream events delivered, every check passing. The
thresholds come from this run. "Before" is the same load before the performance loop below.

| Endpoint | p95 ms before | p95 ms | p50 ms | p99 ms |
|---|---|---|---|---|
| agents.create | 11.1 | 7.5 | 1.6 | 14.5 |
| agents.list | 9.3 | 7.7 | 2.3 | 10.6 |
| events.send | 11.6 | 8.3 | 1.9 | 68.5 |
| files.list | 6.4 | 5.2 | 1.5 | 9.4 |
| files.metadata | 1.3 | 1.1 | 0.6 | 2.3 |
| files.upload | 34.1 | 36.0 | 19.6 | 58.9 |
| sessions.create | 27.1 | 24.1 | 3.3 | 45.8 |
| sessions.delete | 25.4 | 20.4 | 3.9 | 34.8 |
| sessions.list | 8.7 | 9.5 | 2.2 | 128.8 |
| skills.download | 10.4 | 10.5 | 4.9 | 15.4 |
| skills.list | 6.8 | 2.6 | 0.9 | 7.8 |

## What limits it

The performance loop ran at 5 x the defaults (100 crud/s, 200 events/s, 200 streams), changing one thing per
iteration and reading agentd's telemetry each time:

| Iteration | Change | Requests/s | Dropped iterations | p95 ms (events.send, sessions.create, files.upload, agents.list) | SQL statements per request | Most requests waiting for a connection | Kept |
|---|---|---|---|---|---|---|---|
| 0 | none | 898 | 462 | 147, 179, 232, 101 | 4.13 | 79 (all 10 in use) | |
| 1 | pool of 32 connections | 865 | 762 | 272, 316, 1095, 187 | 4.03 | 338 (all 32 in use) | no: more connections made every statement slower |
| 2 | no `SELECT 1` on each connection checkout | 899 | 431 | 168, 173, 284, 103 | 2.07 | 151 | yes: half the statements |
| 3 | diagnostic only: Postgres `synchronous_commit=off` | 928 | 222 | 46, 86, 203, 68 | 2.07 | 185 | no: not durable |

The limit is outside agentd: Postgres's durable commit on this host. Writes are 3 x slower than reads (INSERT
p95 23 ms against SELECT 8 ms), and turning off the commit flush alone cut p95 by 50 to 75%. Docker Desktop's
virtualized disk makes each WAL flush slow. Iterations 1 and 2 each gained under 5% in throughput, and CPU was
never the limit (about 2.4 of the VM's 5 CPUs in use at 5 x).

`PG_SYNC_COMMIT=off perf/run.sh` reproduces the diagnostic, and `AGENTD_DB_POOL_SIZE` sizes the pool; neither is
for real measurements.
