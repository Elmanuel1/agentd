# Requirements: runtime parity

**Date:** 2026-11-08
**Format:** EARS (Easy Approach to Requirements Syntax). Each requirement uses one pattern:

| Pattern | Template |
|---|---|
| Ubiquitous | The `<system>` shall `<response>`. |
| Event-driven | When `<trigger>`, the `<system>` shall `<response>`. |
| State-driven | While `<state>`, the `<system>` shall `<response>`. |
| Unwanted behaviour | If `<condition>`, then the `<system>` shall `<response>`. |
| Optional feature | Where `<feature>`, the `<system>` shall `<response>`. |

**Systems:** *agentd* (the one binary, in either role), *the API* (agentd in its `api` role: requests, streams,
the work queue), *the loop* (agentd in its `loop` role: steps), *the scheduler* (agentd's schedule claims). Workers are not agentd: they run in the container
project (see Out of scope).

**Terms:**
- **Live:** Anthropic's hosted Managed Agents API, the behaviour agentd matches.
- **Worker:** the official SDK's `EnvironmentWorker`, which runs a session's tools and answers with
  `user.tool_result`.
- **Work item:** the queue entry that tells a worker a session needs it; its id is the session id.
- **Work secret:** the base64 JSON a poller receives with a work item; it carries the work token.
- **Work token:** the work item's `sessions_token`, the only credential a worker holds; it reaches its own session
  and work item only.
- **Environment key:** an `sk-ant-oat01-` Bearer token that lets a poller poll one queue.
- **Poller:** the process that polls a queue with an environment key and starts a worker per work item
  (`ant beta:worker poll --on-work`).
- **Runtime queue:** the work queue of `cloud` sessions, polled with our runtime environment key.
- **Step:** one pass of a session's loop: read the session's event log, build the prompt, call the model, append the
  resulting events. A tool call ends a step.
- **Step queue:** the Postgres table of sessions with a step to run; a **loop worker** is an agentd replica in the
  `loop` role, which claims and runs steps.

**Build order:** work API (W) → event log (F, E) → steps and the loop (S, L) → files for workers (FO) → scheduler
(K) → webhooks (H) → removals (RM) → docs (DOC).

Source decisions: [decisions.md](decisions.md). Evidence: [dependencies.md](dependencies.md). The decision record:
[ADR 0001](adr/0001-agent-loop-in-agentd.md).

## 1. Work API

| ID | Requirement |
|---|---|
| W-1 | The API shall sign work tokens with one key held in Secrets Manager (`<prefix>/system/work_token_key`), created on first boot. |
| W-20 | Where `AGENTD_WORK_TOKEN_KEY` is set, the API shall sign work tokens with it instead. |
| W-2 | The API shall keep work items, leases and hand-outs in Postgres. |
| W-3 | When a session's work is queued, the API shall use the session id as the work item id, one item per session, reused for every run. |
| W-4 | The API shall serve the work object with `actor: null` and set `started_at` when the item is queued. |
| W-5 | When a poller polls, the API shall hand each queued item to one poller. |
| W-6 | If an item handed out is not acknowledged within 5 s, then the API shall hand it out again. |
| W-7 | When a heartbeat has no `desired_ttl_seconds`, or 0, the API shall grant 300 s. |
| W-21 | When a heartbeat's `desired_ttl_seconds` is any other integer, the API shall grant it and echo it as given. |
| W-8 | If `desired_ttl_seconds` is not an integer, then the API shall answer 400 `invalid_request_error` "desired_ttl_seconds: Input should be a valid integer, unable to parse string as an integer". |
| W-9 | When a `stopping` item's lease runs out, the API shall serve it as `stopped`. |
| W-10 | While an `active` item gets no heartbeats, the API shall keep serving it as `active`. |
| W-11 | When a session is archived, deleted or terminated, the API shall move its item to `stopping`, or `stopped` when no worker holds it, and keep the item. |
| W-12 | The API shall issue the work secret with live's keys and values: `version`, `session_ingress_token`, `api_base_url`, `sources`, `auth`, `sessions_token`, `claude_code_args`, `environment_variables`, `use_code_sessions`. |
| W-13 | When `agentd environment-keys create` runs, the API shall store only the key's peppered digest in `environment_keys`, bound to one organization, workspace and environment, and print the `sk-ant-oat01-` key once. |
| W-14 | When the API boots with no runtime key in Secrets Manager, the API shall mint one, bound to the runtime queue. |
| W-15 | If an environment key arrives as `x-api-key`, then the API shall answer 401 `authentication_error` "invalid x-api-key". |
| W-16 | If an environment key calls a route other than its queue's `work` list, `poll`, `stats`, `ack`, `heartbeat`, `stop`, or a session's `GET`, events, stream and `POST …/events`, then the API shall answer 403 `permission_error` with live's scope message. |
| W-17 | If an environment key reads a session of another environment, then the API shall answer 404. |
| W-18 | If a workspace API key calls `work/poll`, then the API shall answer 401 `authentication_error` "Invalid bearer token". |
| W-19 | While a session is idle with `requires_action`, the API shall refuse every event but `user.tool_confirmation`, `user.custom_tool_result`, `user.tool_result` and `user.interrupt`, with live's 400 text naming the waiting ids. |

## 2. Event log

| ID | Requirement |
|---|---|
| F-1 | When an event is appended or stamped processed on any replica, the API shall send it to every open stream of its session on every replica, in log order. |
| F-2 | When a session is deleted, the API shall end its streams on every replica with `session.deleted`. |
| F-3 | When a session is archived, the API shall end its streams on every replica. |
| E-1 | Where a workspace is listed for DynamoDB, the API shall keep its new sessions' events in DynamoDB behind the same port, with the same order, take and read semantics, and keep each session in the store it was created in. |

## 3. Steps and the loop

| ID | Requirement |
|---|---|
| S-1 | When a client event, `user.tool_result` or `user.custom_tool_result` is appended, the API shall put the session on the step queue in the same commit. |
| S-2 | The loop shall claim a due step with `FOR UPDATE SKIP LOCKED` under a lease of 12 min, longer than the step's 10 min model deadline, so that two loop workers never run one session's step at once. |
| S-3 | The loop shall read the session's log, build the prompt and call the model with no transaction open, then in one commit release its claim, stamp the client events it read processed, and append its events. |
| S-4 | If a loop worker's commit finds its claim gone (a takeover), then the loop shall roll the commit back. |
| S-5 | When a claim's lease runs out before its commit, the loop shall let another loop worker take the step over and run it again from the log. |
| S-6 | When a session is woken while its step runs, the loop shall run another step after the commit. |
| S-7 | The loop shall hold no session state in a replica's memory between steps and nothing while a tool runs. |
| R-1 | agentd shall run in the role its `AGENTD_ROLE` setting names: `api`, `loop`, or `all`. |
| M-1 | The loop shall run at most `AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE` model calls at once per workspace. |
| M-2 | When the model provider answers 408, 409, 429 or 5xx, the loop's model client shall retry with backoff and jitter, waiting at least the `retry-after` header when present, up to `AGENTD_MODEL_ATTEMPTS` inside the step's deadline. |
| M-3 | Where the provider supports prompt caching, the loop shall mark the system prompt and the tool list cacheable. |
| L-1 | The loop shall build each model call's prompt from the session's event log. |
| L-13 | The loop shall apply each tool's permission policy, writing `agent.tool_use` with its evaluation. |
| L-14 | When the model calls a custom tool, the loop shall write `agent.custom_tool_use` and wait for the caller's `user.custom_tool_result`. |
| L-15 | When the agent delegates, the loop shall run the delegate on a thread of its own and record its thread events. |
| L-16 | The loop shall add each model call's usage to the session and record `session.usage`, and stop with `budget_reached` once the budget is spent. |
| L-17 | The loop shall record `agent.thinking`, `span.model_request_start` and `span.model_request_end` for each model call. |
| L-2 | When the model asks for a built-in tool on a `self_hosted` session, the loop shall write `agent.tool_use` and idle the thread and the session `requires_action` (`event_ids`), in the order `session.thread_status_idle`, `session.usage`, `session.status_idle`. |
| L-3 | When a `self_hosted` session's `user.tool_result` arrives, the loop shall write `session.status_running` and `session.thread_status_running` and continue. |
| L-4 | While a `cloud` session waits on built-in calls, the loop shall keep it `running` and record each `user.tool_result` as `agent.tool_result`. |
| L-5 | The loop shall run the outcome grader on a thread of its own between `span.outcome_evaluation_start` and `_end`, its built-in calls answered by a worker without idling the session, and take one more primary turn after the evaluation. |
| L-6 | When the first `user.interrupt` arrives with an unanswered built-in call, the loop shall post `agent.tool_result` `is_error: true` "Tool execution was interrupted before completion. Please retry." and idle `end_turn`. |
| L-7 | When a second `user.interrupt` arrives, the loop shall force-stop the session's work. |
| L-8 | When a step needs a worker, the loop shall queue the session's work only when its item is `stopped` or not yet queued: `self_hosted` work on its environment's queue, `cloud` work on the runtime queue. |
| L-9 | If a work token posts anything but `user.tool_result` or `user.custom_tool_result` for its session's unanswered calls, then the API shall refuse it. |
| L-10 | If a work token reads a file, skill or memory store other than its session's files, its agent's skills and its session's memory stores, then the API shall refuse it. |
| L-11 | If a work token writes a memory store its session attaches `read_only`, then the API shall answer 403. |
| L-12 | If a work token calls after its item is `stopped`, then the API shall answer 401. |

## 4. Files for workers

| ID | Requirement |
|---|---|
| FO-1 | When a session's work token downloads a file its session mounts or names as an outcome rubric, the API shall serve its content, which no API key can download. |
| FO-2 | When a session's work token uploads a file with `scope_id` set to its session, the API shall keep it as that session's downloadable output, listed by `files.list(scope_id=...)`. |
| FO-3 | If a session's work token deletes a file other than its session's outputs, then the API shall answer 404. |

## 5. Scheduled work

| ID | Requirement |
|---|---|
| K-1 | The scheduler shall keep deployment schedules in Postgres, so that a schedule registered on one replica fires on any. |
| K-2 | When a schedule is due, the scheduler shall claim it and set its next fire in one transaction, keeping the fire contract: one attempt, no backfill, exactly once per target and time. |

## 6. Webhooks

| ID | Requirement |
|---|---|
| H-1 | When a session's status changes to running, idle or terminated, the API shall send each enabled endpoint configured for its organization live's body (`whe_<32 hex>`, six-digit `created_at`, `data: {type, id, organization_id, workspace_id}`). |
| H-2 | The API shall sign deliveries with Standard Webhooks and send `user-agent: Claude-Platform`. |
| H-3 | If a delivery is not answered 2xx, then the API shall retry it twice about 31 s apart, re-signed, then drop it. |
| H-4 | The API shall keep pending deliveries in Postgres, so that they survive a restart. |

## Industry practice

| Pattern | Source | Here |
|---|---|---|
| A server-side agent loop with remote tool execution | Anthropic Managed Agents: the loop on the server, `EnvironmentWorker` runs tools and posts results | Followed exactly. |
| A job queue in Postgres claimed with `FOR UPDATE SKIP LOCKED` | Postgres docs (`SELECT … FOR UPDATE SKIP LOCKED`); queues built on it: Que, GoodJob, Oban, pg-boss, River | Followed for the step queue, the work queue and schedules. |
| Durable execution: state in a log, each step replayable | Temporal (workflow history replayed by any worker), AWS Step Functions | Followed: the event log is the history; a step derives its action from it, so a rerun is safe. |
| A lease with an expiry instead of a held connection or lock | The SRE review of 2026-11-06; Temporal activity timeouts | Followed; a commit checks its claim is still its own. |
| Fan-out by signal, data from the source of truth | Common pub/sub notify-then-read designs | Followed with Valkey signals and log reads. |

Departures: none.

## Plan of PRs

One stack with `gh stack`; spec, migration and code are separate PRs; each deploys alone and changes nothing on
stage or prod until its switch PR. Points at most 4. Migration numbers are taken at merge time. "Reuses" names
work already written on local branches.

| # | PR | Kind | Pts | Requirements | Reuses |
|---|---|---|---|---|---|
| P0 | This spec | spec | 1 | all | |
| W1 | The work-token key lives in Secrets Manager | code | 2 | W-1 | |
| W2m | `work_items` table | migration | 1 | W-2 | |
| W3 | Postgres work store, unused | code | 3 | W-2, W-5, W-6, W-9 | |
| W4 | Work items live in Postgres; id = session id; `actor`; `started_at` | code | 3 | W-2 to W-4, W-7, W-8, W-10, W-11 | |
| W5 | The work secret carries live's fields | code | 1 | W-12 | |
| W6m | `environment_keys` table | migration | 1 | W-13 | branch `environment-keys` |
| W7 | Environment keys: store, CLI, runtime key, edge rules, poll by environment key only | code | 4 | W-13 to W-18 | branch `environment-keys` |
| W8 | A session waiting on a tool refuses other events | code | 1 | W-19 | |
| F1 | Stamping in one statement; ordered reads by `seq` | code | 2 | S-3 | `update_processed_at` (PR #44) |
| F2 | Valkey signals; each replica relays its sessions' streams | code | 3 | F-1 to F-3 | |
| X1 | Store config moves to `config/store.rs` | code | 1 | none (makes room) | |
| E1 to E3 | DynamoDB event log: writes, reads, the flag | code | 3 each | E-1 | |
| Sm | `loop_steps` table | migration | 1 | S-1, S-2 | |
| L1 to L5 | Engine restored from `16805c8^`: model client, history, turn with parked built-in calls, threads, grading, budget (unused) | code | 3 to 4 each | L-1 to L-7, L-13 to L-17, M-2, M-3 | |
| S1 | Step queue store: enqueue, claim, commit, takeover (unused) | code | 3 | S-1 to S-6 | |
| S2 | Loop workers: one step per claim; `AGENTD_ROLE`; model concurrency per workspace (unused) | code | 3 | S-3, S-7, R-1, M-1 | |
| L6 | Work tokens: answers for open calls only, reads and writes scoped, die with the work | code | 3 | L-9 to L-12 | memory guard (branch `harness-memory-stores`), token scoping tests (PR #44) |
| FO1 | Workers' file reads and output uploads with the work token | code | 2 | FO-1 to FO-3 | files session paths (PR #44) |
| L7 | Sessions run on agentd's loop; their work is queued and agentd launches nothing (flagged per workspace) | code | 3 | L-2 to L-4, L-8 | branch `self-hosted-work` |
| Km | `schedules` table | migration | 1 | K-1 | |
| K1 | Postgres scheduler | code | 3 | K-1, K-2 | |
| Hm | `webhook_deliveries` | migration | 1 | H-4 | |
| H1 | Session status webhooks, signed, retried from Postgres | code | 3 | H-1 to H-4 | |
| RM1 | Remove the harness, the `/harness/*` routes and the harness CI job | code | 2 | ADR consequences | |
| RM2 | Remove agentd's sandbox launch: `launch_sandbox`, the `Runtime` wiring, `sandbox-docker`, `sandbox-worker` | code | 3 | ADR consequences | |
| RM3 | Remove the egress proxy (`egress`, `egress-proxy`) and the model-key egress binding, once `agentd-worker`'s proxy has its credential source (Open, below) | code | 2 | ADR consequences | |
| DOC1 | Docs and runbook | docs | 1 | all | |

34 PRs including the spec.

### Per-PR detail

Each code PR lists its files, its statements per operation (sql-transactions rule 1), and its tests. Database tests
are gated by `AGENTD_PG_TESTS`, as today.

- **W1.** `secrets-aws/src/system_secret.rs` (new, about 55 lines) holds the read-or-create body that `pepper.rs`
  has today; `work_token_key()` and `api_key_pepper()` call it. Boot: 1 `GetSecretValue`, 1 `CreateSecret` the first
  time; per request none. Tests (LocalStack): the second read returns the first's bytes; a token minted under one
  boot verifies under the next.
- **W2m.** `create table work_items (workspace_id, session_id, org_id, environment_id, queue, state, metadata,
  created_at, acknowledged_at, started_at, latest_heartbeat_at, last_heartbeat, lease_until, handed_out_at,
  stop_requested_at, stopped_at, actor, request_id, primary key (workspace_id, session_id))`; indexes
  `(queue, created_at) where state = 'queued'` and `(workspace_id, environment_id, created_at desc)`. `actor` and
  `request_id` default from the connection settings, as every table since history PR #41.
- **W3.** `work/src/store.rs` (port, about 60 lines); `store-postgres/src/entities/work_items.rs` (about 180).
  Statements: queue 1 INSERT; claim, one transaction, 1 SELECT of the oldest queued item of the queue not handed out
  in 5 s `FOR UPDATE SKIP LOCKED LIMIT 1` and 1 UPDATE of `handed_out_at`; ack 1 `UPDATE … WHERE state = 'queued'
  RETURNING *`, else 1 SELECT; heartbeat 1 `UPDATE … WHERE state NOT IN ('stopping','stopped') AND
  coalesce(last_heartbeat, 'NO_HEARTBEAT') = $expected RETURNING *`, else 1 SELECT; stop 1 UPDATE; reaper, a task per
  replica every 5 s, 1 UPDATE of `stopping` items past their lease; stats 1 SELECT with `count(*) FILTER`. Tests:
  two claims take different items; an unacknowledged item is claimable after 5 s; a stale heartbeat is refused;
  `stopping` past its lease reads `stopped`, `active` stays `active`.
- **W4.** `work/src/board.rs` removed; `service.rs` and `lifecycle.rs` call the store; `Work.id` is the session id.
  Tests: a new `AppState` on the same database serves the item (restart); `"actor":null`; `started_at` while
  queued; an archived session's item reads `stopping`.
- **W5.** `work/src/tokens.rs` `secret()` writes live's nine keys in live's order. Test: the decoded secret's keys,
  order and types.
- **W6m.** `create table environment_keys (environment_key_id, org_id, workspace_id, environment_id, name,
  key_digest unique, partial_key_hint, created_at, revoked_at)`.
- **W7.** `api-keys/src/environment.rs`, `store-postgres/src/entities/environment_keys.rs`,
  `api/src/cli/environment_keys.rs`, `api/src/edge/reach.rs` (what each scoped credential reaches),
  `edge/auth.rs` (resolution only), `work/src/routes.rs` (poll). Per request 1 SELECT by digest, as for workspace
  keys. Tests (`api/tests/edge.rs`, `api/tests/environment_keys.rs`): one per W-13 to W-18 with live's bodies.
- **W8.** `sessions/src/service/events.rs`: the session row is already read; no new statement. Tests: refused with
  live's text; the four answers accepted.
- **F1.** `entities/session_events.rs`: append is one multi-row INSERT; `update_processed_at(ws, session,
  up_to_seq, at)` is one `UPDATE … SET doc = jsonb_set(doc, '{processed_at}', …) WHERE … AND seq <= $up_to AND
  doc->>'processed_at' IS NULL RETURNING seq, doc`; reads by keyset on `seq`, the log's insertion order, which
  orders each session's events. No migration: `processed_at` stays in `doc`, as today. Tests: two stamps get
  disjoint events; a stamp leaves a status event unstamped; keyset reads return each event once.
- **F2.** `sessions/src/service/events.rs` and a `signals` adapter over Valkey; per replica one log read per tick
  for every signalled session it streams. Tests: an event appended on one replica reaches another's stream in
  order; a dropped signal is recovered by the 1 s timer; delete ends the other replica's stream.
- **X1, E1 to E3.** As the 2026-11-06 plan: `config/store.rs`; `crates/adapters/events-dynamodb` with one
  `TransactWriteItems` per append and a conditional update per stamp; the port's test suite run on both stores; the
  workspace list setting. Tests: the port suite on both stores; a listed workspace's new session lands in DynamoDB.
- **Sm.** `create table loop_steps (workspace_id, session_id, due_at, wakes, lease_owner, lease_until, attempts,
  primary key (workspace_id, session_id))`; partial index on `due_at` where `lease_until is null`; index on
  `lease_until`.
- **L1 to L5.** `crates/engine` restored from `16805c8^` (each file under 200 lines): L1 model clients and router,
  the one retry layer (M-2), cache marks (M-3); L2 history, prompt, permission policies; L3 the turn, built-in
  calls parked as `agent.tool_use`; L4 threads, delegation, interrupts (L-6, L-7); L5 grading on its own thread,
  budget and usage through `record.rs`. No statements of their own; the step (S2) reads and writes. Tests: the old
  engine's unit tests over a fake contract, plus a parked call, an interrupt pair with live's text, and the budget
  cases.
- **S1.** `sessions/src/steps.rs` (port) and `store-postgres/src/entities/loop_steps.rs`. Statements in the next
  section. Tests (`store-postgres/tests/pg_steps.rs`): two workers race for one step and only one runs it; a worker
  killed after the model call and before its commit, the step reruns after its lease and its events are appended
  once; a lease that runs out is taken over and the late commit is rolled back; a wake during a step runs another
  step.
- **S2.** `crates/engine/src/worker.rs` (claim, run one step, commit), `api/src/config/role.rs` (`AGENTD_ROLE`,
  `AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE`, `AGENTD_MODEL_ATTEMPTS`). Tests: an `api` replica claims no step; two
  `loop` replicas share the queue; a workspace at its limit leaves its next step for later and others run.
- **L6.** `api/src/edge/reach.rs` (token routes); the memory routes' guard asks sessions which stores a session
  attaches and how; every token request reads its item (1 SELECT by primary key). Tests: one per L-9 to L-12.
- **FO1.** `files/src/session.rs` (output upload, delete, download of mounted files), `SessionsService::reads_file`
  on `Session::mounts` and `UserEvent::rubric_file`. Download: 1 SELECT of the session, 1 SELECT of its events when
  the file is not mounted, 1 SELECT of the file, 1 object GET. Tests (`api/tests/session_token.rs`): one per FO-1
  to FO-3.
- **L7.** `sessions/src/service/sandbox.rs`: a session launches nothing; its work is queued at create on its
  environment or the runtime queue; the flag lists workspaces. Tests: the live poll-path cases replayed with `ant`
  pollers and the scripted model.
- **Km.** `create table schedules (target, workspace_id, spec, next_fire_at, paused, primary key (target))`; index on
  `next_fire_at` where not paused.
- **K1.** `crates/adapters/scheduler-postgres` replaces `scheduler-local` in `backends.rs`. Tick, per replica every
  10 s, one transaction: 1 `SELECT … WHERE next_fire_at <= now() AND NOT paused FOR UPDATE SKIP LOCKED LIMIT 100`;
  1 `UPDATE schedules SET next_fire_at = v.next FROM unnest($targets, $nexts) AS v(target, next) WHERE
  schedules.target = v.target` (no backfill); then the fires, outside it; `claim_fire` stays the
  exactly-once guard. Tests: two schedulers on one database fire each occurrence once; a schedule registered on one
  fires on the other.
- **Hm.** `webhook_deliveries` (the configured endpoint's key and URL, body, attempt, `next_attempt_at`,
  `claimed_until`, `delivered_at`, `dropped_at`).
- **H1.** `crates/resources/webhooks`: endpoints read from configuration (D20); one multi-row INSERT of deliveries
  per status change in its commit; a task per replica claims due rows 100 at a time `FOR UPDATE SKIP LOCKED`, signs,
  POSTs with a 10 s timeout, and marks them in one UPDATE per batch. Tests: signature verified with the SDK's
  `client.beta.webhooks.unwrap()`; a 401 receiver sees three attempts with one `webhook-id`; a due delivery
  survives a restart.
- **RM1.** Removes `harness/`, `api/src/harness.rs`, `HarnessContract` writes used only by the harness, the harness
  CI job. CI's `sdk` and `demo` jobs run agentd's loop with the scripted model and the container project's worker.
- **RM2, RM3.** Remove the crates and wiring named in the table once L7 is on everywhere; they move to the container
  project (Out of scope).

### The step queue: statements per operation (S1, S2)

| Operation | Statements |
|---|---|
| Enqueue, in the commit that appends the waking event | 1 `INSERT INTO loop_steps … ON CONFLICT (workspace_id, session_id) DO UPDATE SET wakes = loop_steps.wakes + 1` |
| Claim, one transaction | 1 `SELECT … WHERE due_at <= now() AND (lease_until IS NULL OR lease_until < now()) ORDER BY due_at FOR UPDATE SKIP LOCKED LIMIT 1`; 1 `SELECT pg_advisory_xact_lock(hashtext($workspace))`; 1 `SELECT count(*) FROM loop_steps WHERE workspace_id = $1 AND lease_until > now()`; at the limit, 1 `UPDATE loop_steps SET due_at = now() + interval '1 second' WHERE workspace_id = $1 AND lease_until IS NULL` defers the whole workspace and the claim ends; under it, 1 `UPDATE … SET lease_owner = $me, lease_until = now() + interval '12 minutes', attempts = attempts + 1` |
| Run, no transaction open | 1 keyset SELECT of the session's events, noting the highest `seq` read; the model call |
| Commit, one transaction | 1 `DELETE FROM loop_steps WHERE … AND lease_owner = $me AND attempts = $claimed AND wakes = $claimed_wakes`; when it deletes nothing, 1 `UPDATE … SET lease_owner = NULL, lease_until = NULL, due_at = now() WHERE … AND lease_owner = $me AND attempts = $claimed`, and when that updates nothing too the claim was taken over: roll back; 1 `UPDATE session_events SET doc = jsonb_set(doc, '{processed_at}', …) WHERE … AND seq <= $read_up_to AND doc->>'processed_at' IS NULL`; 1 multi-row INSERT of the step's events; the session row writes those events cause, as `record` does today |
| Takeover | the Claim statements: a lease that has run out is claimable |

## Rollout

- W4 ships without a flag: on `main` no client polls work, so it changes only agentd's own harness work, verified
  on stage.
- L7 sits behind a setting listing workspaces; a session's path is fixed at create. Stages: one internal
  workspace, then 10%, 50%, 100% of workspaces, each for at least 24 h including the daily peak, judged on the SLIs
  below, with an absolute stop when a page fires.
- E3's DynamoDB log follows the same stages, separately.
- A workspace is listed for L7 only once `agentd-worker` polls the runtime queue in that deployment environment;
  until then its `cloud` sessions would wait on a queue nobody polls.
- RM1 to RM3 merge after L7 reaches 100% and `agentd-worker` serves `cloud` work on stage and prod.
- Per deployment environment, the settings (`AGENTD_ROLE`, model limits, the L7 and E3 workspace lists) live in its
  configuration.

## Out of scope

- **`agentd-worker`**, the container project (its own repository, D6): the image running the official `EnvironmentWorker`
  unchanged; the Docker launcher for development and the Fargate launcher for production; the file sidecar (files
  staged in, outputs and `/workspace` snapshots out, with no AWS credentials in the task); the egress proxy with credential placeholders; the
  `--on-work` command; our `cloud` deployment polling the runtime queue. It speaks only the public API, so it runs
  against live as well. It stages files in and saves outputs through the public files endpoints with the work token
  (FO-1, FO-2); snapshots go to its own bucket through presigned URLs its launcher makes. Its plan is a separate
  document. `sandbox-docker`, `sandbox-worker` and the egress crates
  move there (RM2, RM3). (The relay binary in `sandbox-worker`, also named `agentd-worker`, moves with it.)
- Session webhook registration through an API: live has none (B8).

## Open

These need the user's answer before the PRs they name start.

- **Files for workers (FO-1, FO-2).** Live refuses a `self_hosted` session's file resources and has no endpoint
  through which a worker stages them; Anthropic stages them inside its own `cloud` sandboxes. For
  `agentd-worker` to serve `cloud`, agentd lets a session's work token download the files its session mounts and
  upload its outputs through the public files endpoints, which live does not allow. Recommendation: accept this as
  the one departure, limited to the work token and the session's own files.
- **Credentials for the egress proxy (RM3).** Live has no public API that returns vault credential values.
  Recommendation: our `cloud` deployment of `agentd-worker` reads them from Secrets Manager with its own IAM role; a
  customer's `self_hosted` deployment is given its own. RM3 waits on this.

## SRE

### SLOs (targets in `docker/observability/slos.yaml`)

| Component | SLI | SLO |
|---|---|---|
| API | non-5xx / all requests | 99.9% |
| Step start | steps claimed within 1 s of their wake / all steps | 99% |
| Tool-call dispatch | `agent.tool_use` on the worker's stream within the threshold / all built-in calls | 90% < 250 ms, 99% < 1 s |
| Event delivery | events on every open stream within 1 s of append | 99% |
| Turn success | turns ending `end_turn`, `requires_action` or `budget_reached` / all turns | 99% |

### Sizing at the peak

| Resource | Calculation | Result |
|---|---|---|
| Steps | 10^4 sessions / 20 s per step (N1, N2) | 500 steps/s |
| Event appends | 500 steps/s × about 10 events per step (N3) | 5×10^3 events/s, one multi-row INSERT per step |
| Step statements | about 9 per step | 4.5×10^3/s on Postgres |
| A workspace at its limit | 3 statements per deferral, at most once a second per workspace | bounded by the number of workspaces at their limit |
| Log reads | 500 steps/s × the log size a step reads | measured in the load test; a read replica serves them when it grows |
| Model calls | 500/s at most, 32 at once per workspace (D13) | bounded by M-1 |
| Loop workers | steps in flight = 500/s × model latency; each worker runs up to 64 steps at once (a setting) | about 240 workers at a 30 s latency; the autoscaler follows queue depth |

### Load test plan

`perf/` (k6) drives N sessions against the scripted model with 2 to 8 loop workers, and records step-start latency,
claim contention, Postgres statements per second, connections, the log size per step and model latency. Runs: a
steady run; a loop worker killed mid-step; a workspace at its model limit. Results go on the S2 PR.

## Audit of this plan

Inventory: the 2026-11-06 inventory of the runtime on 88ab967 (a first pass by reading `work`, `sessions`,
`edge/auth.rs`, `sandbox_work.rs`, `harness.rs` and the migrations; a second, independent pass of 35 patterns, from
`WorkScope` with 22 hits to `SKIP LOCKED` with 0; the reconciliation added the harness reading the 412 detail at the
wrong path, interrupts drained in FIFO order, the conformance suites polling with a workspace key, and three area
audits' findings). A second pass over `git diff 88ab967 origin/main -- crates harness docker` (21 files, all in
`store-postgres`, `docker/sandbox/README.md`) changes no runtime file; it adds that new tables record their writer
through the connection settings from history PR #41.

Removal sites for RM2 and RM3 (first pass: launch, runtime and egress identifiers, 23 files and about 100 hits;
second pass: workspace members and dependencies, 13 hits; the second pass added `docker/sandbox/Dockerfile`
building `sandbox-worker` and the api crate's dependencies on `egress-proxy` and `sandbox-docker`):

| Site | Today | Plan |
|---|---|---|
| `api/src/sandbox_work.rs` (12 hits) | issues work and egress settings for each sandbox | RM2 removes it; L7 queues work instead |
| `api/src/egress.rs`, `api/src/egress/admin.rs` (17) | registers sessions with the egress service | RM3 |
| `api/src/config/sandbox.rs`, `config/runtime.rs` (14) | sandbox and runtime settings | RM2 |
| `api/src/state.rs`, `backends.rs` (15) | wire the runtime, the issuer and egress | RM2, RM3 |
| `sessions/src/service.rs`, `service/sandbox.rs`, `service/lifecycle.rs`, `lib.rs`, `record.rs`, `harness.rs` (13) | launch and stop sandboxes; `SandboxWork` | L7 replaces launch with queueing; RM2 removes the rest |
| `vaults/src/egress.rs` (4) | hands vault credentials to the egress service | RM3, after the open question below |
| `crates/egress/` (14), `crates/adapters/egress-proxy/` (2) | the egress service and proxy | move to `agentd-worker` (RM3) |
| `crates/adapters/sandbox-docker/` (9), `crates/sandbox-worker/` | the Docker runtime and relay | move to `agentd-worker` (RM2) |
| `Cargo.toml` members and deps, `api/Cargo.toml`, `docker/sandbox/Dockerfile` | build them | RM2, RM3 |

| Source | Result |
|---|---|
| `~/.claude/CLAUDE.md` | PASS: separate spec, migration and code PRs; `gh stack`; no footer; no em dashes; never push `main` or merge. |
| `rules/planning-phase.md`, `plan-audit` | PASS: inventory, second pass, plan, this audit, questions to the user one at a time. |
| `rules/planning-questions.md` | PASS: every new table is a migration PR the user approves; access checks read one row; no file over 200 lines grows (W7 splits `edge/auth.rs` into `reach.rs`). |
| `rules/diagnose-before-patch.md` | PASS: the engine is restored, not rewritten; container code leaves agentd instead of growing beside it. |
| `rules/comment-policy.md` | PASS: no comments planned beyond the SQL claim notes. |
| `rules/auto-rerun-ci.md` | Not applicable: applies during the build. |
| Repo `CLAUDE.md` | Not applicable: agentd has none. |
| Memory: agentd parity, no SDK of our own, harness in sandbox | PASS: the loop in agentd, workers outside, no agentd-only route. |
| `grill-me` ledger | PASS: no `ASSUMED` row. |
| `industry-practice` | PASS: section above; no departure. |
| `review/references/code-design.md` and its four parts | PASS: questions on models (`Session::mounts`, `UserEvent::rubric_file`), no `else`, files at most 200 lines. |
| `review/references/sql-transactions.md` | PASS: statements per operation; no subqueries; claims with `SKIP LOCKED`; a commit checks its claim first. |
| `review/references/architecture.md`, `system-design.md` | PASS: one path for `cloud` and `self_hosted`; no parallel route. |
| `review/references/checks.md`, script checks | PASS on this text; each PR runs them. |
| `build` (migrations, dependencies) | PASS: migrations in their own PRs; `aws-sdk-dynamodb` beside the other AWS SDK crates. |
| `test` | PASS: each requirement row names its test in its PR. |
| `tech-writing-style`, `no-em-dashes` | PASS: terms defined above; no em dashes; the design stated as fact. |
| `pr-body` | PASS: each PR's body carries these IDs. |
| `tosspaper-testing` | Not applicable as written (tosspaper's stage); each agentd PR records its local and CI verification. |
| SRE skills | PASS: SLOs, sizing, load test plan, rollout stages, the 2026-11-06 review's findings answered (signals over Valkey, no held lease, one retry layer, reaper as a task, autoscaling on queue depth). |
