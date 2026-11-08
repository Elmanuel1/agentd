# Runtime parity: dependencies

Each fact the design rests on, with its evidence. "Live" means Anthropic's hosted Managed Agents API. `VERIFIED`
rows were read in the official SDK's source or measured on live. `ACCEPTED-RISK` rows are the user's numbers, accepted
by the user, each with the signal that reopens it.

## What Anthropic's worker uses (official SDK 2.3.0)

| # | Fact | Status | Evidence |
|---|---|---|---|
| A1 | The worker's per-item entry is `client.beta.environments.work.worker(workdir=...).handle_item()`, reading `ANTHROPIC_WORK_ID`, `ANTHROPIC_ENVIRONMENT_ID`, `ANTHROPIC_SESSION_ID`, `ANTHROPIC_WORK_SECRET`; it needs no environment key when the secret carries a sessions token. | VERIFIED | `lib/environments/_worker.py` `handle_item` and its docstring |
| A2 | Poll and ack belong to the poller, with the environment key; `ant beta:worker poll --on-work` (the SDK's docstring calls it `ant worker poll`) hands a claimed item to a fresh process. | VERIFIED | `lib/environments/_poller.py` (`work.poll`, `work.ack`, `work.stop`), `_worker.py` docstring; `ant beta:worker --help` (ant 1.39.0) lists `poll` |
| A3 | Per item the worker calls `work.heartbeat`, `work.stop`, `sessions.retrieve`, `skills.versions.retrieve` and `download`, `memory_stores.memories` `list`, `retrieve`, `create`, `update`, `delete`, and `sessions.events` `stream`, `list`, `send`. | VERIFIED | `_worker.py`, `lib/tools/_beta_session_runner.py`, `_skills.py`, `_memories.py` |
| A4 | The worker sends only `user.tool_result` and `user.custom_tool_result`; it reads `agent.tool_use`, `agent.custom_tool_use`, `user.tool_confirmation`, `session.thread_status_*`, `session.status_idle`, `session.status_terminated`, `session.deleted`. | VERIFIED | `_beta_session_runner.py` lines 189, 360-369, 610-710, 736-739, 1089 |
| A5 | The memory endpoints accept the item's sessions token and refuse the environment key. | VERIFIED | `_worker.py`, comment above `SessionMemoryStores` |
| A6 | With only the default toolset the worker leaves `agent.custom_tool_use` pending for the caller. | VERIFIED | `_beta_session_runner.py` (a runner claims only tools it holds); spike, 2026-11-04 |

## What live does (probed 2026-11-05 to 2026-11-07)

| # | Fact | Status | Evidence |
|---|---|---|---|
| B1 | A work item's id is its session id, reused per run; `actor: null`; `started_at` set while queued. | VERIFIED | `ant-poller-spike.md`, "Live Managed Agents, poll path" |
| B2 | The work secret's keys: `version`, `session_ingress_token`, `api_base_url`, `sources`, `auth`, `sessions_token`, `claude_code_args`, `environment_variables`, `use_code_sessions`. | VERIFIED | same |
| B3 | An environment key is Bearer only (`x-api-key` gets 401 "invalid x-api-key"); off-scope routes get 403 `permission_error` "OAuth token does not meet scope requirement any_of(org:sessions:claude_code, user:developer, user:sessions:claude_code, workspace:developer)"; a workspace key on poll gets 401 "Invalid bearer token". | VERIFIED | runtime-parity probes, 2026-11-07 |
| B4 | Heartbeat TTL: default and 0 give 300 s; any other integer is echoed; a non-integer is a 400 with live's text. | VERIFIED | same |
| B5 | On `self_hosted`, each built-in call idles the session `requires_action`; on `cloud` the session stays `running` and results show as `agent.tool_result`. | VERIFIED | same |
| B6 | The grader runs on its own thread, its calls answered by the worker, inside `span.outcome_evaluation_*`. | VERIFIED | same |
| B7 | While a tool call is unanswered, `user.message` is refused with live's 400 text; `user.interrupt` posts a synthetic error result. | VERIFIED | `ant-poller-spike.md` |
| B8 | Session webhook endpoints have no registration API (Console only). | VERIFIED | probes, 2026-11-07 |
| B9 | "MCP credentials (`mcp_oauth`, `static_bearer`): each credential is keyed by an `mcp_server_url` ... When the agent connects to a server at that URL at session runtime, the token is injected automatically." | VERIFIED | platform.claude.com/docs/en/managed-agents/vaults |
| B10 | "When multiple vaults contain a matching credential, the first vault with a match wins." A session orders its vaults only by `vault_ids`. | VERIFIED | same page |
| B11 | "When no MCP credential matches by `mcp_server_url`, the connection is attempted unauthenticated and will error if the server requires authentication." | VERIFIED | same page |
| B12 | "Credentials are re-resolved periodically ... credential rotation, archival, or deletion takes effect in running sessions without a restart." Anthropic refreshes an expired `mcp_oauth` token with its `refresh` block; a failed refresh emits `vault_credential.refresh_failed` (a webhook event in SDK 2.3.0, `types/beta/beta_webhook_vault_credential_refresh_failed_event_data.py`). | VERIFIED | same page; SDK 2.3.0 |
| B13 | MCP events and errors: `agent.mcp_tool_use` (`name`, `input`, `mcp_server_name`, `evaluated_permission`), `agent.mcp_tool_result` (`mcp_tool_use_id`, `content`, `is_error`), `mcp_authentication_failed_error` and `mcp_connection_failed_error` (`mcp_server_name`, `message`, `retry_status`). | VERIFIED | SDK 2.3.0 `types/beta/sessions/beta_managed_agents_agent_mcp_tool_*`, `beta_managed_agents_mcp_*_error.py` |

## Load (sizing inputs)

| # | Fact | Status | Evidence |
|---|---|---|---|
| N1 | 10^4 sessions active at once at the peak. | ACCEPTED-RISK | user, 2026-11-06: "10,000 sessions at once"; accepted with the plan, user, 2026-11-08: "Approve the plan". Reopens when stage or prod shows more than 5×10^3 active sessions. |
| N2 | About one loop step every 20 s per active session. | ACCEPTED-RISK | user, 2026-11-08 ("about one step every 20 seconds per session"); accepted with the plan, user, 2026-11-08: "Approve the plan". Reopens when the load test or stage measures more than one step per 10 s per session. |
| N3 | About 10 events and 2 status changes per built-in call; 2 KB per event. | VERIFIED | live capture, Haiku with fast tools |

Measured, not assumed: the log size a step reads and the model's latency per call. The design does not rest on
either; the load test plan in [requirements](requirements.md) measures both and the loop workers' autoscaler
follows queue depth.
