# Load tests (k6)

`perf/agentd.js` drives agentd the way the official SDKs do (their headers, betas and request shapes), in four
scenarios running together:

| Scenario | What it does | Load (defaults) |
|---|---|---|
| `crud` | agents create and list, sessions create and list, files list, skills list, in rotation | `RATE` = 20 per second |
| `events` | `user.message` events sent to 20 existing sessions | 2 x `RATE` per second |
| `streams` | `STREAMS` concurrent event streams; each opens a stream, sends 5 events, deletes the session and checks every event arrived in order, ending with `session.deleted` | `STREAMS` = 50 |
| `files` | 1 MB uploads, metadata reads, and skill bundle downloads (uploaded files are not downloadable, as live) | `RATE` / 4 per second |

No model is called (agentd's step 1 runs no engine) and nothing reaches AWS: the stack is agentd, Postgres 16 and
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
(5 events 500 ms apart, then the delete), so its p95 bound is 3.5 s.

## Baseline

Docker Desktop on Apple silicon (5 CPUs, 10 GB for the VM), defaults (20 crud/s, 40 events/s, 50 streams, 5 file
rounds/s, 2 minutes): 26,628 requests, 0 failed, 11,000 stream events delivered, every check passing.

| Endpoint | p50 ms | p95 ms | p99 ms |
|---|---|---|---|
| agents.create | 1.8 | 11.1 | 29.3 |
| agents.list | 2.5 | 9.3 | 15.3 |
| events.send | 2.4 | 11.6 | 36.6 |
| files.list | 1.7 | 6.4 | 11.1 |
| files.metadata | 0.6 | 1.3 | 4.8 |
| files.upload (1 MB) | 18.5 | 34.1 | 57.5 |
| sessions.create | 5.8 | 27.1 | 38.3 |
| sessions.delete | 6.9 | 25.4 | 35.2 |
| sessions.list | 2.4 | 8.7 | 16.1 |
| skills.download | 4.9 | 10.4 | 20.8 |
| skills.list | 1.1 | 6.8 | 15.6 |

## What limits it

At 5 x the defaults (100 crud/s, 200 events/s, 200 streams): 886 requests/s with no failures, but p95 rose to
100 to 460 ms and k6 dropped 379 iterations it could not start on time. agentd's own telemetry shows why:

- The Postgres connection pool is the limit. All 10 connections were in use and up to 87 requests waited for one
  (`db.client.connection.pending_requests`), at about 5.2 SQL statements per request.
- CPU was not: agentd used 0.78 of a CPU, Postgres 0.67, LocalStack 0.80 and k6 0.89, about 3.1 of the VM's 5.
- The load generator shares those 5 CPUs, so numbers above this level measure the laptop as much as agentd.
