# Runtime parity: requirements and plan

agentd's runtime matches Anthropic Managed Agents: the agent loop runs in agentd and each sandbox runs Anthropic's
worker unchanged ([ADR 0001](adr/0001-agent-loop-in-agentd.md), [decisions](decisions.md),
[dependencies](dependencies.md)).

Systems: **agentd** (its API role and its loop-worker role, one binary), **the dispatcher** (`agentd-dispatch`, the
`--on-work` command agentd's poller runs), **the worker** (Anthropic's SDK `EnvironmentWorker`, never changed).

Terms:
- A **step** is one pass of a session's loop: read the session's event log, build the prompt, call the model,
  append the resulting events. A step ends when the model's reply is recorded; a tool call ends the step.
- The **step queue** is the Postgres table of sessions with a step to run.
- A **loop worker** is an agentd replica in the `loop` role, which claims and runs steps.
- The **runtime queue** is the work queue `cloud` sessions wait on, polled by agentd's own poller.

Build order: W, then F and E, then L and S, then K, then C, then H, then D (the plan below).

## Requirements (EARS)

Work API (W):

| ID | Requirement |
|---|---|
| W-1 | agentd shall sign work tokens with one key held in Secrets Manager (`<prefix>/system/work_token_key`), created on first boot; `AGENTD_WORK_TOKEN_KEY` keeps priority when set. |
| W-2 | agentd shall keep work items, leases and hand-outs in Postgres, so that a restart or another replica loses no item and invalidates no token. |
| W-3 | When a session's work is queued, agentd shall use the session id as the work item id, one item per session, reused for every run. |
| W-4 | agentd shall serve the work object with `actor: null` and set `started_at` when the item is queued. |
| W-5 | When a poller polls, agentd shall hand each queued item to one poller (`FOR UPDATE SKIP LOCKED`), and hand it out again when it is not acknowledged within 5 s. |
| W-6 | When a heartbeat's `expected_last_heartbeat` does not match the stored one, agentd shall answer 412 as today. |
| W-7 | When a heartbeat has no `desired_ttl_seconds`, or 0, agentd shall grant 300 s; any other integer shall be granted and echoed as given; a non-integer shall get 400 `invalid_request_error` "desired_ttl_seconds: Input should be a valid integer, unable to parse string as an integer". |
| W-8 | When a `stopping` item's lease runs out, agentd shall read it as `stopped`; an `active` item whose heartbeats stop shall stay `active`. |
| W-9 | When a session is archived, deleted or terminated, agentd shall move its item to `stopping` (`stopped` when no worker holds it), never delete it. |
| W-10 | agentd shall issue the work secret with live's keys and values: `version`, `session_ingress_token`, `api_base_url`, `sources`, `auth`, `sessions_token`, `claude_code_args`, `environment_variables`, `use_code_sessions`. |
| W-11 | When `agentd environment-keys create` runs, agentd shall store only the key's peppered digest in `environment_keys`, bound to one organization, workspace and environment, and print the `sk-ant-oat01-` key once; agentd shall mint its own runtime key at boot into Secrets Manager. |
| W-12 | When an environment key arrives as `x-api-key`, agentd shall answer 401 `authentication_error` "invalid x-api-key". |
| W-13 | When an environment key calls a route other than its environment's `work` list, `poll`, `stats`, `ack`, `heartbeat`, `stop`, or a session's `GET`, events, stream and `POST …/events`, agentd shall answer 403 `permission_error` with live's scope message; another environment's session shall be 404. |
| W-14 | When a workspace API key calls `work/poll`, agentd shall answer 401 `authentication_error` "Invalid bearer token". |
| W-15 | While a session is idle with `requires_action`, agentd shall refuse every event but `user.tool_confirmation`, `user.custom_tool_result`, `user.tool_result` and `user.interrupt`, with live's 400 text naming the waiting ids. |

Session events (F, E):

| ID | Requirement |
|---|---|
| F-1 | When an event is appended or stamped processed on any replica, agentd shall send it to every open stream of its session on every replica, in log order. |
| F-2 | When a step takes a session's client events, agentd shall stamp `processed_at` in the same statement that selects them, so that one step takes each event once. |
| F-3 | When a session is deleted, agentd shall end its streams on every replica with `session.deleted`; archiving shall end them. |
| E-1 | Where a workspace is listed for DynamoDB, agentd shall keep its new sessions' events in DynamoDB behind the same port, with the same order, take and read semantics; a session's store is fixed at create. |

The loop (L) and its steps (S):

| ID | Requirement |
|---|---|
| L-1 | agentd shall run each session's loop: model calls, history from the event log, permission policies, custom tools, threads and delegation, outcomes and grading, budget and `session.usage`, `agent.thinking`, `span.model_request_*` and thread status events. |
| L-2 | On a `self_hosted` session, when the model asks for a built-in tool, agentd shall write `agent.tool_use` and idle the thread and the session `requires_action` (`event_ids`), in the order `session.thread_status_idle`, `session.usage`, `session.status_idle`, until the `user.tool_result` arrives; then `session.status_running` and `session.thread_status_running`. |
| L-3 | On a `cloud` session, agentd shall keep the session `running` through built-in calls and record the worker's `user.tool_result` as `agent.tool_result`. |
| L-4 | agentd shall run the outcome grader on a thread of its own between `span.outcome_evaluation_start` and `_end`, its built-in calls answered by the worker without idling the session, and take one more primary turn after the evaluation. |
| L-5 | When the first `user.interrupt` arrives with an unanswered built-in call, agentd shall post `agent.tool_result` `is_error: true` "Tool execution was interrupted before completion. Please retry." and idle `end_turn`; a second shall force-stop the session's work. |
| L-6 | When a run needs the worker, agentd shall queue the session's work only when its item is `stopped` or not yet queued. |
| L-7 | A work token shall post only `user.tool_result` and `user.custom_tool_result` for its session's unanswered calls, read only its session's files, its agent's skills and its session's memory stores, write only stores attached `read_write`, and stop working once its item is `stopped`. |
| L-8 | When a session is in a `self_hosted` environment, agentd shall start no sandbox; its work shall wait on the environment's queue for the customer's pollers. |
| S-1 | When a client event, `user.tool_result` or `user.custom_tool_result` is appended, agentd shall put the session on the step queue in the same commit. |
| S-2 | A loop worker shall claim a due step with `FOR UPDATE SKIP LOCKED` under a lease with an expiry, so that two workers never run the same session's step at once. |
| S-3 | A step shall read the session's log, build the prompt, call the model and append its events and release its claim in one commit; agentd shall hold no session state in a replica's memory between steps and nothing while a tool runs. |
| S-4 | When a claim's lease expires before its commit, another loop worker shall take the step over and run it again; a step shall be safe to repeat (it derives its action from the log). |
| S-5 | When a session is woken while its step runs, agentd shall run another step after the commit, so that no wake is lost. |
| R-1 | agentd shall run in the role its `AGENTD_ROLE` setting names: `api` (requests and streams), `loop` (steps), or `all`. |

Model provider limits (M):

| ID | Requirement |
|---|---|
| M-1 | agentd shall run at most the configured number of model calls at once per workspace (`AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE`); a step over the limit waits on the queue. |
| M-2 | When the model provider answers 408, 409, 429 or 5xx, the model client shall retry with backoff and jitter, honouring `retry-after`, up to the configured attempts inside the step's deadline; no other layer retries a model call. |
| M-3 | Where the provider supports prompt caching, agentd shall mark the system prompt and the tool list cacheable. |

Scheduled work (K):

| ID | Requirement |
|---|---|
| K-1 | agentd shall keep deployment schedules in Postgres, so that a schedule registered on one replica fires on any. |
| K-2 | When a schedule is due, one replica shall claim it and advance its next fire in one statement, keeping the fire contract (attempts 1, no backfill, exactly once per `(target, scheduled_at)`). |

The cloud runtime (C):

| ID | Requirement |
|---|---|
| C-1 | `cloud` sessions' work shall wait on the runtime queue, polled by agentd's `ant beta:worker poll --on-work agentd-dispatch` with the runtime key. |
| C-2 | The dispatcher shall start one sandbox per session through the `Runtime` port whose entry is the SDK's `worker(workdir="/workspace").handle_item()`, with the work secret as a 0600 file, the environment's variables and init script, its image when it has one, and today's limits. |
| C-3 | The dispatcher shall stage the session's `file` resources read-only at `/mnt/session/uploads/<mount_path>` before the worker starts. |
| C-4 | When the session idles `end_turn` or `budget_reached`, and at exit, the dispatcher shall upload new or changed files under `/mnt/session/outputs/` as session-scoped outputs. |
| C-5 | The dispatcher shall restore `/workspace` from the session's last snapshot before the worker starts, and snapshot it when the session idles `end_turn` or `budget_reached` and at exit. |
| C-6 | The sandbox's traffic shall leave only through agentd's egress pool, whose proxy password carries the session's signed policy; the sandbox reaches only the egress pool and agentd. |
| C-7 | The dispatcher shall stop sandboxes it no longer has work for, including after its own crash. |

Webhooks (H):

| ID | Requirement |
|---|---|
| H-1 | When a session's status changes to running, idle or terminated, agentd shall send each enabled endpoint of its organization the live body (`whe_<32 hex>`, six-digit `created_at`, `data: {type, id, organization_id, workspace_id}`). |
| H-2 | agentd shall sign deliveries with Standard Webhooks and send `user-agent: Claude-Platform`. |
| H-3 | When a delivery is not answered 2xx, agentd shall retry it twice about 31 s apart, re-signed, then drop it; pending deliveries survive a restart. |

## Plan of PRs

One stack with `gh stack`; spec, migration and code are separate PRs; each deploys alone with no change to stage or
prod until its switch PR. Migration numbers are taken at merge time. "Reuses" names work already written on local
branches (not pushed).

| # | PR | Kind | Requirements | Reuses |
|---|---|---|---|---|
| P0 | This spec | spec | all | |
| W1 | The work-token key lives in Secrets Manager | code | W-1 | |
| W2 | `work_items` table | migration | W-2 | |
| W3 | Postgres work store, unused | code | W-2, W-5, W-7, W-8 | |
| W4 | Work items live in Postgres; id = session id; `actor`; `started_at` | code | W-2 to W-4, W-6, W-9 | |
| W5 | The work secret carries live's fields | code | W-10 | |
| W6 | `environment_keys` table | migration | W-11 | branch `environment-keys` (V9) |
| W7 | Environment keys: store, CLI, edge, poll by environment key only, live's 403 text | code | W-11 to W-14 | branch `environment-keys` |
| W8 | A session waiting on a tool refuses other events | code | W-15 | |
| F1m | `session_seq`, `processed_at` column, lz4 | migration | F-2 | |
| F1 | Take stamps in one statement; keyset reads | code | F-2 | `update_processed_at` (PR #44) |
| F2 | Valkey signals; each replica relays its sessions' streams | code | F-1, F-3 | |
| X1, E1 to E3 | Store config split; DynamoDB event log (flagged) | code | E-1 | |
| Sm | `loop_steps` table | migration | S-1, S-2 | |
| L1 to L5 | Engine restored from `16805c8^`: model client, history, turn with parked built-in calls, threads, grading, budget (unused) | code | L-1 to L-5, M-2, M-3 | |
| S1 | Step queue store: enqueue, claim, release, takeover (unused) | code | S-1 to S-5 | |
| S2 | Loop workers: one step per claim over the engine; `AGENTD_ROLE`; model concurrency per workspace (unused) | code | S-3, R-1, M-1 | |
| L6 | Work tokens: results for open calls, attached memory, read-only stores, die with the work | code | L-7 | memory guard (branch `harness-memory-stores`), session-token scoping tests (PR #44) |
| L7 | `self_hosted` sessions run on agentd's loop and the customer's pollers (flagged) | code | L-2, L-5, L-6, L-8 | branch `self-hosted-work` |
| Km | `schedules` table | migration | K-1 | |
| K1 | Postgres scheduler: claim due fires per tick | code | K-1, K-2 | |
| C1 | The work secret carries the sandbox's settings; signed egress policy | code | C-6 | |
| C2 | Worker image (official `anthropic` only) and `agentd-dispatch`, Docker adapter | code | C-2, C-3, C-7 | |
| C2b | ECS Fargate adapter | code | C-2, C-7 | |
| C3 | Dispatcher syncs outputs and snapshots | code | C-4, C-5 | |
| C4 | agentd's poller pool | code | C-1 | |
| C5 | `cloud` sessions run on agentd's loop and the runtime queue (flagged) | code | C-1, L-3 | |
| C6 | Remove the harness, `/harness/*` routes, agentd's Docker launch, the harness CI job | code | L-1 | |
| Hm | `webhook_deliveries` (and endpoints once O1 is answered) | migration | H-3 | |
| H1 | Session status webhooks, signed, retried from Postgres | code | H-1 to H-3 | |
| D1 | Docs and runbook | docs | all | |

PR Elmanuel1/agentd#44 is superseded by this plan.

### Step queue: statements per operation (S1, S2)

`loop_steps (workspace_id, session_id, due_at, wakes, lease_owner, lease_until, attempts)`, primary key
`(workspace_id, session_id)`, a partial index on `due_at` where `lease_until IS NULL`, and one on `lease_until`.

| Operation | Statements |
|---|---|
| Enqueue (in the commit that appends the waking event) | 1 `INSERT … ON CONFLICT (workspace_id, session_id) DO UPDATE SET wakes = loop_steps.wakes + 1` |
| Claim (one transaction) | 1 `SELECT … WHERE due_at <= now() AND (lease_until IS NULL OR lease_until < now()) ORDER BY due_at FOR UPDATE SKIP LOCKED LIMIT 1`; 1 `SELECT count(*) … WHERE workspace_id = $1 AND lease_until > now()` (M-1); 1 `UPDATE … SET lease_owner, lease_until = now() + $ttl, attempts = attempts + 1` |
| Run | 1 keyset `SELECT` of the session's events; the model call (no transaction open) |
| Commit (one transaction) | 1 multi-row `INSERT` of the step's events; session row writes the events cause (status, usage), as `record` does today; 1 `DELETE … WHERE … AND wakes = $claimed_wakes`; when it deletes no row (a wake arrived), 1 `UPDATE … SET lease_until = NULL, due_at = now()` |
| Takeover | the Claim statements: an expired lease is claimable |

Tests (`store-postgres/tests/pg_steps.rs`, Postgres gated): two workers racing for one step, only one claims it; a
worker killed after the model call and before the commit, the step reruns after its lease expires and appends its
events once; a lease that expires is taken over by another worker; a wake during a step runs another step.

## SRE

### SLOs (targets in `docker/observability/slos.yaml`, signed off 2026-11-06, plus S)

| Component | SLI | SLO |
|---|---|---|
| API | non-5xx / all requests | 99.9% |
| Step start | steps claimed within 1 s of their wake / all steps | 99% |
| Tool-call dispatch | `agent.tool_use` on the worker's stream within the threshold / all built-in calls | 90% < 250 ms, 99% < 1 s |
| Event delivery | events on every open stream within 1 s of append | 99% |
| Turn success | turns ending `end_turn`, `requires_action` or `budget_reached` / all turns | 99% |
| Sandbox start (cloud) | runtime work acknowledged within 30 s of queueing | 99% |

### Sizing at the peak

| Resource | Calculation | Result |
|---|---|---|
| Steps | 10^4 sessions / 20 s (S2) | 500 steps/s |
| Step statements | about 8 per step | 4×10^3/s on Postgres |
| Log reads | 500 steps/s × 400 KB (S4) | 200 MB/s, from a read replica when it grows |
| Event appends | 10^4 × 5 events/s | 5×10^4/s, multi-row per step |
| Model calls | 500/s at most, 32 per workspace (O2) | bounded by M-1 |
| Loop workers | a step holds no connection during the model call (5 to 30 s); 64 steps in flight per worker | 500 × 30 s / 64 = about 240 workers at the worst model latency |

### Load test plan

`perf/` (k6) drives N sessions against the scripted model with 2 to 8 loop workers: step-start latency, claim
contention, Postgres statements per second and connections; a run that kills a loop worker mid-step; a run at the
per-workspace limit. Results and the before and after table go on the S2 PR.

## Audit of this plan

- Inventory: the 2026-11-06 inventory on 88ab967 (two passes, 35 patterns, reconciled) holds on `main` at 63e07b7.
  The second pass over the delta (`git diff 88ab967 origin/main -- crates harness docker`) changes no runtime
  file; it adds that new tables record their writer through the connection settings from #41 (`actor`,
  `request_id` defaults).
- `~/.claude/CLAUDE.md`: PASS. Separate spec, migration and code PRs, `gh stack`, no footer, no em dashes.
- `planning-questions.md`: PASS. Every new table is a migration PR the user approves; access checks read one row.
- `diagnose-before-patch`: PASS. The engine is restored, not rewritten; the harness and its routes go.
- `sql-transactions`: PASS. Statements listed per operation; no subqueries; claims with `SKIP LOCKED`; the commit
  that appends also releases.
- SRE (`sre-non-abstract-design`, `sre-simplicity`, `sre-managing-load`): PASS with S2 and S4 to measure; the
  2026-11-06 review's findings are answered (signals over Valkey, no held lease, snapshots only at `end_turn`, one
  retry layer, reaper as a task, autoscaling on queue depth).
- Memory `project-agentd-harness-in-sandbox` (parity) and `feedback-agentd-no-own-sdk`: PASS.
- Open: O1 (webhook registration), O2 (model limit defaults).
