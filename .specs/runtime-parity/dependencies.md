# Runtime parity: dependencies

Each fact the design rests on, with its evidence. `VERIFIED` rows were read in source or measured on live;
`ASSUMED` rows wait on a check named in the row.

## What Anthropic's worker uses (official SDK 2.3.0)

| # | Fact | Status | Evidence |
|---|---|---|---|
| A1 | The worker's per-item entry is `client.beta.environments.work.worker(workdir=...).handle_item()`, reading `ANTHROPIC_WORK_ID`, `ANTHROPIC_ENVIRONMENT_ID`, `ANTHROPIC_SESSION_ID`, `ANTHROPIC_WORK_SECRET`; it needs no environment key when the secret carries a sessions token. | VERIFIED | `lib/environments/_worker.py` `handle_item` and its docstring ("a pod that must never hold the environment key runs on the secret's sessions token alone") |
| A2 | Poll and ack are the poller's, with the environment key; `ant worker poll --on-work` hands a claimed item to a fresh process. | VERIFIED | `_poller.py` (`work.poll`, `work.ack`, `work.stop`), `_worker.py` docstring |
| A3 | Per item the worker calls: `work.heartbeat`, `work.stop`, `sessions.retrieve`, `skills.versions.retrieve` and `download`, `memory_stores.memories` `list`, `retrieve`, `create`, `update`, `delete`, and `sessions.events` `stream`, `list`, `send`. | VERIFIED | `_worker.py`, `tools/_beta_session_runner.py`, `tools/_skills.py`, `tools/_memories.py` |
| A4 | The worker sends only `user.tool_result` and `user.custom_tool_result`; it reads `agent.tool_use`, `agent.custom_tool_use`, `user.tool_confirmation`, `session.thread_status_*`, `session.status_idle`, `session.status_terminated`, `session.deleted`. | VERIFIED | `_beta_session_runner.py` lines 189, 360-369, 610-710, 736-739, 1089 |
| A5 | The memory endpoints accept the item's sessions token and refuse the environment key. | VERIFIED | `_worker.py` comment above `SessionMemoryStores` |
| A6 | With only the default toolset the worker leaves `agent.custom_tool_use` pending for the caller. | VERIFIED | `_beta_session_runner.py`: a runner claims only tools it holds; spike 2026-11-04 ("tool not owned by this runner; leaving the tool_use_id pending") |

## What live does (probed 2026-11-05 to 2026-11-07)

| # | Fact | Status | Evidence |
|---|---|---|---|
| L1 | Work item id is the session id, reused per run; `actor: null`; `started_at` set while queued. | VERIFIED | `ant-poller-spike.md`, "Live Managed Agents, poll path" |
| L2 | The work secret's keys: `version`, `session_ingress_token`, `api_base_url`, `sources`, `auth`, `sessions_token`, `claude_code_args`, `environment_variables`, `use_code_sessions`. | VERIFIED | same |
| L3 | An environment key is Bearer only (`x-api-key` gets 401 "invalid x-api-key"); off-scope routes get 403 `permission_error` "OAuth token does not meet scope requirement any_of(org:sessions:claude_code, user:developer, user:sessions:claude_code, workspace:developer)"; a workspace key on poll gets 401 "Invalid bearer token". | VERIFIED | runtime-parity probes 2026-11-07 |
| L4 | Heartbeat TTL: default and 0 give 300 s; any other integer is echoed; a non-integer is a 400 with live's text. | VERIFIED | same |
| L5 | On `self_hosted`, each built-in call idles the session `requires_action`; on `cloud` the session stays `running` and results show as `agent.tool_result`. | VERIFIED | same |
| L6 | The grader runs on its own thread, its calls answered by the worker, inside `span.outcome_evaluation_*`. | VERIFIED | same |
| L7 | While a tool call is unanswered, `user.message` is refused with live's 400 text; `user.interrupt` posts a synthetic error result. | VERIFIED | `ant-poller-spike.md` |
| L8 | Session webhook endpoints have no registration API (Console only). | VERIFIED | probes 2026-11-07; open question O1 |

## Load (SRE sizing inputs)

| # | Fact | Status | Evidence |
|---|---|---|---|
| S1 | 10^4 sessions active at once at the peak. | VERIFIED (user decision) | user, 2026-11-06 (Q10) |
| S2 | About one loop step every 20 s per active session. | ASSUMED | user, 2026-11-08; measured on stage before the loop's switch PR |
| S3 | 10 events and 2 status changes per built-in call; 2 KB per event. | VERIFIED | live capture, Haiku with fast tools |
| S4 | A step's log read is under 400 KB (about 200 events). | ASSUMED | checked on the scripted demo's sessions before the loop's switch PR |
| S5 | OpenRouter and Anthropic answer 429 with `retry-after`; Anthropic models accept `cache_control` through OpenRouter. | ASSUMED | read the providers' docs during the model-client PR |
