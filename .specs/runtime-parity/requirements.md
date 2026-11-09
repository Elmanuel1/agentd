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
the work queue), *the loop* (agentd in its `loop` role: steps), *the scheduler* (agentd's schedule claims). Section
11 records `agentd-worker`'s side of the contract, built in its own repository: *the launcher* (starts a sandbox per
work item), *the swap* (attaches `environment_variable` credentials at egress: the Cloudflare launcher's Durable
Object, or the optional AWS egress proxy) and *the fleet* (the Fargate launcher's tasks). Workers are not agentd: they
run in the container project (see Out of scope).

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
- **Window:** a model's `max_input_tokens` (C4). **Checkpoint:** the `agent.thread_context_compacted` event that
  holds a thread's summary and the last event it covers.
- **Placeholder:** the signed stand-in for an `environment_variable` credential that a `cloud` sandbox sees in place
  of the value (D28).

**Build order:** work API (W, EV) → event log (F, E) → steps and the loop (S, L, M) → compaction (CP) → files for
workers (FO) → MCP (MC) → vaults (VA) → scheduler (K) → webhooks (H) → conformance (J) → removals (RM) → docs (DOC).

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
| W-16 | If an environment key calls a route other than W-22's (runtime key only), its queue's `work` list, `poll`, `stats`, `ack`, `heartbeat`, `stop`, or a session's `GET`, events, stream and `POST …/events`, then the API shall answer 403 `permission_error` with live's scope message. |
| W-17 | If an environment key reads a session of another environment, then the API shall answer 404. |
| W-18 | If a workspace API key calls `work/poll`, then the API shall answer 401 `authentication_error` "Invalid bearer token". |
| W-22 | When agentd's runtime key calls `GET /v1/environments/{id}` for an environment with a work item on the runtime queue, the API shall serve the environment as it does to a workspace key (D39). |
| W-23 | If the runtime key calls `GET /v1/environments/{id}` for an environment with no work item on the runtime queue, then the API shall answer 403 `permission_error` with live's scope message. |
| W-19 | While a session is idle with `requires_action`, the API shall refuse every event but `user.tool_confirmation`, `user.custom_tool_result`, `user.tool_result` and `user.interrupt`, with live's 400 text naming the waiting ids. |

## 2. Event log

| ID | Requirement |
|---|---|
| F-1 | When an event is appended or stamped processed on any replica, the API shall send it to every open stream of its session on every replica, in log order. |
| F-2 | When a session is deleted, the API shall end its streams on every replica with `session.deleted`. |
| F-3 | When a session is archived, the API shall end its streams on every replica. |
| F-4 | While a stream waits for a signal, the API shall also read its session's log on a 5 s timer with up to 1 s of random jitter, so that a dropped signal delays an event by at most about 6 s. |
| F-5 | While Postgres is overloaded (a connection waits more than 1 s for the pool, or the database's connections pass 90% of `max_connections`), the API shall refuse a new event stream with 529 `overloaded_error` "Overloaded" and keep the streams already open. |
| E-2 | Where an event retention period is set (`AGENTD_EVENT_RETENTION_DAYS`, unset by default), the API shall delete the events of sessions archived longer ago than that period. |
| E-4 | When a session is deleted, the API shall delete its events through the event-log port after the commit that deletes the session row. |
| E-3 | Where a read replica is configured (`AGENTD_PG_READ_URL`), the API shall serve `events.list` from it; steps, streams and every write read the primary. |

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
| S-8 | A loop worker shall run at most `AGENTD_STEPS_PER_WORKER` steps at once (default 256), from one database pool of `AGENTD_PG_POOL_SIZE` connections (default 16), holding a connection only while it claims, reads or commits. |
| R-1 | agentd shall run in the role its `AGENTD_ROLE` setting names: `api`, `loop`, or `all`. |
| R-2 | agentd shall export the number of due, unclaimed steps (`agentd_loop_steps_due`) and of leased steps (`agentd_loop_steps_leased`) as metrics, which the loop workers' autoscaler follows. |
| M-1 | The loop shall run at most `AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE` model calls at once per workspace. |
| M-2 | When the model provider answers 408, 409, 429 or 5xx, the loop's model client shall retry with backoff and jitter, waiting at least the `retry-after` header when present, up to `AGENTD_MODEL_ATTEMPTS` inside the step's deadline. |
| M-3 | Where the provider supports prompt caching, the loop shall mark the system prompt and the tool list cacheable. |
| M-4 | The loop shall hold an account-wide budget per model provider of requests and input tokens per minute (`AGENTD_MODEL_RPM`, `AGENTD_MODEL_ITPM`) in Valkey; when a step would pass it, the loop shall defer the step to the next minute without calling the model. |
| L-1 | The loop shall build each model call's prompt from the session's event log. |
| L-13 | The loop shall apply each tool's permission policy, writing `agent.tool_use` with its evaluation. |
| L-14 | When the model calls a custom tool, the loop shall write `agent.custom_tool_use` and wait for the caller's `user.custom_tool_result`. |
| L-15 | When the agent delegates, the loop shall run the delegate on a thread of its own and record its thread events. |
| L-16 | The loop shall add each model call's usage to the session and record `session.usage`. |
| L-18 | If the session's budget is spent, then the loop shall stop the turn with `budget_reached`. |
| L-19 | Where a session created on the harness path is idle and its workspace is listed for L7, the loop shall serve the session's next turn from its event log, so that no idle session depends on the harness. |
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

## 4. Compaction

Decision D31; evidence C1 to C5.

| ID | Requirement |
|---|---|
| CP-1 | The loop shall estimate a thread's prompt size as the previous model call's input, cache read, cache creation and output tokens plus one token per 4 bytes of the events appended since, with no token-counting call. |
| CP-2 | When a thread's estimated prompt passes the lower of `compaction.clear_at` (`AGENTD_COMPACTION_CLEAR_AT`, default 100,000 tokens) and 50% of the model's window, the loop shall build the prompt with each tool result older than the last `compaction.keep_results` (default 3, from 3 to 5) replaced by a placeholder naming its event id, keeping every tool input and every result with `is_error` true. |
| CP-3 | The loop shall cap each tool result at `tool_output_token_limit` (`AGENTD_TOOL_OUTPUT_TOKEN_LIMIT`, default 16,000 tokens) when it appends the result to the prompt, marking the cut; the stored event stays whole. |
| CP-4 | When a thread's estimated prompt passes 80% of the model's window, the loop shall run a compaction step for that thread before its next turn. |
| CP-5 | The loop shall run a compaction step only between turns, while the thread has no tool call waiting for its result, as the session's one running step (S-2), so that a thread has at most one compaction running. |
| CP-6 | A compaction step shall summarise from the thread's latest checkpoint and the events after it, never the whole log, keeping the user's messages verbatim newest first up to 20,000 tokens (the message that overflows truncated) and the last 2 turns, and asking for the goal, constraints, decisions, files touched, open todos, unresolved errors and next steps. |
| CP-7 | Where the thread's model is an Anthropic model, the compaction step shall call it with the main loop's system prompt and tools, beta `compact-2026-09-04` and `"compaction": {"type": "summarize"}` carrying CP-6's instructions, and every later call of the thread shall send the returned `compaction` block back unmodified, first in `messages`, with the `compact-2026-09-04` beta header (C1). |
| CP-8 | Where the thread's model is not an Anthropic model, the compaction step shall call it with the main loop's system prompt and tools and a handoff prompt carrying CP-6's instructions, and put the summary first in the thread's later prompts. |
| CP-9 | When a compaction step finishes, the loop shall append `agent.thread_context_compacted` as the thread's checkpoint, holding the summary, the last event it covers and the model that wrote it, unique per thread and covered event; the API shall serve the event with live's fields only, and no event is deleted. |
| CP-10 | If a compaction call returns no summary, then the loop shall continue with CP-2's prompt and run the compaction step again at the thread's next step. |
| CP-11 | If a compaction request is itself larger than the window, then the loop shall drop the oldest events after the checkpoint from that request until it fits, and retry. |
| CP-12 | When a thread's model changes to one with a smaller window, or the thread's latest checkpoint was written by another model, the loop shall compact again before the thread's next turn. |
| CP-13 | The loop shall take each model's window from the provider's `GET /v1/models/{id}` `max_input_tokens` (C4), read once per process, or from `AGENTD_MODEL_WINDOWS` for a provider that has no such field. |

## 5. Files for workers

| ID | Requirement |
|---|---|
| FO-1 | When a session's work token downloads a file its session mounts, the API shall serve its content, which no API key can download. |
| FO-2 | When a session's work token uploads a file with `scope_id` set to its session, the API shall keep it as that session's downloadable output, listed by `files.list(scope_id=...)`. |
| FO-3 | If a session's work token deletes a file other than its session's outputs, then the API shall answer 404. |

## 6. Scheduled work

| ID | Requirement |
|---|---|
| K-1 | The scheduler shall keep deployment schedules in Postgres, so that a schedule registered on one replica fires on any. |
| K-2 | When a schedule is due, the scheduler shall claim it and set its next fire in one transaction, keeping the fire contract: one attempt, no backfill, exactly once per target and time. |

## 7. MCP servers

Decisions D26, D27 and D32; evidence B9 to B14, B17, B18, B22.

| ID | Requirement |
|---|---|
| MC-1 | When a session's agent lists an MCP server (`mcp_servers`, `type: "url"`), the loop shall connect to its `url` from agentd, never from a worker. |
| MC-2 | The loop shall authenticate to an MCP server with the `mcp_oauth` or `static_bearer` credential whose `mcp_server_url` equals the server's `url`, taken from the first vault in the session's `vault_ids` that holds one. |
| MC-3 | If no credential in the session's vaults matches a server, then the loop shall connect unauthenticated. |
| MC-4 | If an MCP server refuses the loop's authentication, then the loop shall record `session.error` with `mcp_authentication_failed_error` (`mcp_server_name`, `message`, `retry_status`) and let the session continue. |
| MC-5 | If the loop cannot connect to an MCP server, then the loop shall record `session.error` with `mcp_connection_failed_error` and let the session continue. |
| MC-14 | When an MCP connection has failed, the loop shall not retry it in the same turn, and shall retry it on the session's next `session.status_idle` to `session.status_running` transition (B18). |
| MC-6 | The loop shall read a credential's secret from Secrets Manager through agentd's `SecretStore` once in each step that connects to its server or calls one of its tools, with no cache across steps, so that a rotated, archived or deleted credential applies from the next step (D27, D32). |
| MC-7 | When an `mcp_oauth` credential's access token has expired and it carries a `refresh` block, the loop shall claim the refresh with one guarded UPDATE of the credential row (matching the `updated_at` it read, and only when that is more than 30 s old), then call the block's `token_endpoint`, then store the new token with `PutSecretValue`, then record the new expiry in the row. |
| MC-20 | If an expired token's credential row has `updated_at` less than 30 s old, then the loop shall end its step and run it again 1 s later, with no further read. |
| MC-23 | If the loop's refresh claim matches no row, then the loop shall re-read the credential's row and secret and use the stored token when it has not expired, and otherwise end its step and run it again 1 s later. |
| MC-24 | When the model calls an MCP tool whose evaluation is `allow`, the loop shall commit `agent.mcp_tool_use` with that step and keep the session on the step queue, so that the next step calls the tool. |
| MC-25 | When a step starts with an unanswered `agent.mcp_tool_use` that is allowed, by its evaluation or by a `user.tool_confirmation`, the loop shall call that tool and commit its `agent.mcp_tool_result` in that step with no model call, keeping the session due for the next step, unless MC-26 applies. |
| MC-27 | If an `mcp_oauth` refresh's `token_endpoint`, or a redirect it answers with, resolves to a loopback, private or link-local address, then the loop shall not call it and shall treat the refresh as failed (MC-21, H-5). |
| MC-26 | If the loop cannot call an allowed MCP tool, because its server's connection failed in this turn or fails now, then the loop shall record `agent.mcp_tool_result` with `is_error` true and live's `content` for that call, and, for each later call to that server in the same turn other than the one whose connection attempt MC-5 records, the `mcp_connection_failed_error` `session.error` live records for it (B27), so that no allowed call stays unanswered. |
| MC-21 | If a refresh fails, then the loop shall record `session.error` with `mcp_authentication_failed_error` and let the session continue (B17). |
| MC-8 | When the model calls an MCP tool, the loop shall record `agent.mcp_tool_use` (`name`, `input`, `mcp_server_name`, the permission evaluation from the agent's `mcp_toolset` configuration) and `agent.mcp_tool_result` (`mcp_tool_use_id`, `content`, `is_error`). |
| MC-9 | The loop shall never write a credential's secret into an event, a log line, an output or a work item. |
| MC-10 | If a session's environment has `limited` networking with `allow_mcp_servers` false and an MCP server's host is not in `allowed_hosts`, then the loop shall not connect and shall record `mcp_connection_failed_error`, since the session error types have no egress-blocked error (B14). |
| MC-11 | If an MCP server's URL, or a redirect it answers with, resolves to a loopback, private or link-local address, then the loop shall not connect and shall record `mcp_connection_failed_error`. |
| MC-12 | When an MCP tool call's permission evaluation is `ask`, the loop shall write `agent.mcp_tool_use` and idle the session `requires_action`, call the server only after a `user.tool_confirmation` that allows it, and record an `is_error` `agent.mcp_tool_result` on a deny. |
| MC-13 | The loop shall keep each MCP server's `Mcp-Session-Id` and tool list per agentd session and server in Postgres (`mcp_connections`), and send `initialize` and `tools/list` only when it reconnects (MC-15). |
| MC-15 | The loop shall reconnect to an MCP server when no connection is stored, when the server answers 404 to the stored `Mcp-Session-Id`, when the credential's `updated_at` differs from the one the connection was made with, and on the session's idle-to-running transition. |
| MC-16 | When a server sends `notifications/tools/list_changed` in a response, or the loop reconnects, the loop shall list the server's tools again and store the new list. |
| MC-17 | The loop shall never use one session's `mcp_connections` row for another session or workspace. |
| MC-22 | When a session is deleted, the API shall delete its `mcp_connections` rows in the commit that deletes the session row. |
| MC-18 | When a deployment fires, the API shall check each MCP server host of the deployment's agent against the environment's networking as MC-10 does, before it creates the session. |
| MC-19 | If a deployment fire's MCP host check fails, then the API shall create no session, record the run with `mcp_egress_blocked_error`, and pause the deployment with `paused_reason` `{"type": "error", "error": {"type": "mcp_egress_blocked_error", ...}}` (B22). |
| H-5 | If an `mcp_oauth` refresh fails, then the API shall send the `vault_credential.refresh_failed` webhook to the organization's configured endpoints. |
| H-6 | When a credential is archived, directly or by its vault's archive, the API shall send the `vault_credential.archived` webhook to the organization's configured endpoints (B16). |

## 8. Vaults

Decision D29; evidence B15 to B17, B23.

| ID | Requirement |
|---|---|
| VA-1 | If a session update sets `vault_ids`, then the API shall answer 400 `invalid_request_error` with live's message (B15). |
| VA-2 | If a session create names a vault that is archived or not in its workspace, then the API shall refuse it with live's status and message. |
| VA-3 | When a vault is archived, the API shall archive each of its unarchived credentials and purge their secrets, so that a running session keeps running and resolves no credential from that vault from its next step, and a new session that names it is refused (VA-2, B16). |
| VA-4 | If a credential update changes an `mcp_oauth` credential's `token_endpoint`, then the API shall refuse it with live's status and message (B17). |
| VA-5 | When a credential update sets a new `access_token` without `expires_at`, the API shall keep the stored expiry (B17). |
| VA-6 | When `mcp_oauth_validate` is called for an `mcp_oauth` credential, the API shall send `initialize` to its `mcp_server_url` with its access token and answer `vault_credential_validation` with `valid` on success, `invalid` when the server answers 401 or 403, and `unknown` otherwise, with `validated_at` (B23). |

## 9. Webhooks

| ID | Requirement |
|---|---|
| H-1 | When a session's status changes to running, idle or terminated, the API shall send each enabled endpoint configured for its organization live's body (`whe_<32 hex>`, six-digit `created_at`, `data: {type, id, organization_id, workspace_id}`). |
| H-2 | The API shall sign deliveries with Standard Webhooks and send `user-agent: Claude-Platform`. |
| H-3 | If a delivery is not answered 2xx, then the API shall retry it twice about 31 s apart, re-signed, then drop it. |
| H-4 | The API shall keep pending deliveries in Postgres, so that they survive a restart. |

## 10. Environment-variable credentials and conformance

Decisions D25, D28 and D36.

| ID | Requirement |
|---|---|
| EV-1 | When the API hands out a `cloud` session's work item, it shall set the work secret's `environment_variables` to one entry per unarchived `environment_variable` credential in the session's vaults, keyed by `secret_name` (the first vault in `vault_ids` wins, B10), whose value is the credential's placeholder; for a `self_hosted` session it shall set `environment_variables` to an empty object. |
| EV-2 | The API shall sign each placeholder with one key held in Secrets Manager (`<prefix>/system/egress_key`), created on first boot, over the session, workspace, vault and credential ids. |
| EV-3 | The API shall never put a credential's value in the work secret, an event, a log or a response. |
| EV-4 | The API shall tag each `environment_variable` credential's secret `credential_type=environment_variable` when it creates the secret, and `agentd vault-secrets tag` shall tag the existing ones, so that the swap's AWS policy reaches those secrets only. |
| J-1 | The `sdk` CI job shall run the official Java SDK (`com.anthropic:anthropic-java` 2.52.0) against agentd with only the base URL changed, covering the cases of the Python and TypeScript suites. |

## 11. `agentd-worker`'s side

Built in `agentd-worker`'s repository (D6) and recorded here so both sides build to one contract. agentd pushes
nothing to `agentd-worker` and has no setting naming a launcher (D38).

Every launcher with secret injection on:

| ID | Requirement |
|---|---|
| EG-1 | When the launcher starts a sandbox, it shall set each `environment_variables` entry of the work secret as an environment variable, so the sandbox sees the placeholder and never the value. |
| EG-3 | When a request to a host in the credential's `allowed_hosts` carries a valid placeholder in a location its `injection_location` allows, the swap shall attach the credential's value from its sealed secret and send the request. |
| EG-4 | If a request carries a placeholder for a host outside the credential's `allowed_hosts`, or in a location its `injection_location` does not allow, then the swap shall send the request with the placeholder unchanged, attaching no value, as live does (B28). |
| EG-5 | If a placeholder's signature fails or its session is not the calling session, then the swap shall send the request with the placeholder unchanged, attaching no value. |
| EG-7 | If the swap needs a value it cannot read (Secrets Manager unreachable, the credential gone) and holds no unexpired copy, then it shall refuse the request (fail closed). |
| EG-8 | The swap shall log a credential's id and a fingerprint, never its value. |
| EG-10 | Where its operator turns secret injection on, the swap shall apply EG-3 to EG-8; our `cloud` deployment turns it on, and it is off by default. |

The optional AWS egress proxy (Fargate and EC2 launchers):

| ID | Requirement |
|---|---|
| EG-2 | The proxy shall take the calling session from `Proxy-Authorization: Basic <session id>:<sessions_token>`, checked with `GET /v1/sessions/{id}` and that token, cached at most 60 s per token digest. |
| EG-6 | The proxy shall read a value from Secrets Manager for each request and keep no copy between requests, so that a rotated value applies on the next request. |
| EG-9 | The proxy shall run as its own service with an IAM role allowed only `secretsmanager:GetSecretValue` on secrets tagged `credential_type=environment_variable` and on `<prefix>/system/egress_key`, and `kms:Decrypt` through Secrets Manager only; the sandbox task role holds no secret permission. |

The Cloudflare launcher (our `cloud`, D38):

| ID | Requirement |
|---|---|
| CF-1 | When our runtime poller (`ant beta:worker poll --on-work`, on a small always-on task beside agentd) receives a work item, its `--on-work` command shall hand the item to the Cloudflare launcher Worker over HTTPS, signed with an HMAC key the two share, and the Worker shall start the session's container through `getByName(session_id)`. |
| CF-2 | The container shall run the official `EnvironmentWorker` unchanged (`handle_item()`), from a fresh disk, with no warm pool, stopping after `sleepAfter` (default 10 min) idle; `limited` networking sets `enableInternet = false` and `allowedHosts`, which always holds the host of the work secret's `api_base_url`, the worker's route to its session; `unrestricted` sets `enableInternet = true` with no `allowedHosts`. |
| CF-8 | While a session's container runs, its `EnvironmentWorker` shall be the only process that heartbeats, stops or answers the session's work item; the Durable Object and the launcher Worker shall never call `work.heartbeat` or `work.stop` or send session events. |
| CF-9 | The launcher shall keep the environment key outside the container: the container receives only the item's `ANTHROPIC_WORK_ID`, `ANTHROPIC_ENVIRONMENT_ID`, `ANTHROPIC_SESSION_ID` and `ANTHROPIC_WORK_SECRET` (whose `sessions_token` `handle_item()` uses, A1) and the placeholders. |
| CF-11 | The launcher shall read an environment's networking with the runtime key (W-22), cache it for `networking_cache_seconds` (default 60), and apply the cached value when a container starts; a running container keeps the networking it started with (B26). |
| CF-12 | If the launcher cannot read an environment's networking, then it shall start no container, retry the read with backoff for up to 10 minutes, and then stop the work item with `work.stop`, so that no sandbox ever starts unrestricted; the session then waits on its tool call until a `user.interrupt` ends it (L-6). |
| CF-10 | If the Durable Object cannot read the egress key, then it shall refuse every request that carries a placeholder (fail closed) and pass the others under CF-2's networking. |
| CF-3 | The session's Durable Object shall stage the session's mounted files into the container from `GET /v1/files/{id}/content` with the work token (FO-1) before the worker starts, and upload outputs with `files.upload` scoped to the session (FO-2) when the session finishes a turn and at exit, through `@cloudflare/sandbox`'s `Files`. |
| CF-4 | The Durable Object shall back `/workspace` up to R2 with `@cloudflare/sandbox`'s `DirectoryBackup` when the session finishes a turn and at exit, and restore it on the next start; where that is unavailable, it shall save a tar of `/workspace` to `agentd-worker`'s bucket through a presigned URL instead. |
| CF-5 | The Durable Object's outbound handler (`interceptHttps = true`, the container trusting Cloudflare's ephemeral CA at start) shall apply EG-3 to EG-5 to every outbound request of the container on ports 80 and 443, treating a placeholder for any session but its own as invalid; a placeholder sent on another port reaches its host unchanged, which Cloudflare's handlers do not intercept (G5). |
| CF-6 | The Durable Object shall keep each value and the egress key in its memory for at most 5 minutes, never in Durable Object storage, and drop them on eviction and at session end, so that a rotated, archived or deleted credential applies within 5 minutes. |
| CF-7 | The handler shall read Secrets Manager with `aws4fetch` and one AWS access key held as a Worker secret, never inside the container, belonging to an IAM user allowed only `secretsmanager:GetSecretValue` on secrets tagged `credential_type=environment_variable` and on `<prefix>/system/egress_key`, `kms:Decrypt` through Secrets Manager only, and only from Cloudflare's published IP ranges (`aws:SourceIp`); the key is rotated every 90 days. |

Tests in `agentd-worker`'s plan include: a session with built-in calls runs to `end_turn` against agentd while
agentd records heartbeats for its work item from one client only (CF-8); `env` run by the agent shows no
`sk-ant-oat01-` value (CF-9); with the networking read failing, no container starts and the item reads `stopped`
(CF-12); with the egress key unreadable, a request carrying a placeholder is refused and one without passes (CF-10); with one credential
unreadable, only the requests carrying its placeholder are refused (EG-7); in the Fargate fleet, `env` run by the agent shows no `sk-ant-oat01-` value (FL-1).

The Fargate launcher's fleet (D33):

| ID | Requirement |
|---|---|
| FL-1 | The fleet shall be an ECS service of identical tasks, each serving one session and exiting, so no container serves two sessions; a poller container in the task holds the environment key, takes one work item, and passes only its `ANTHROPIC_WORK_ID`, `ANTHROPIC_ENVIRONMENT_ID`, `ANTHROPIC_SESSION_ID` and `ANTHROPIC_WORK_SECRET` to the worker container. |
| FL-2 | A job shall read `GET /v1/environments/{env}/work/stats` every 60 s and set the service's desired count to queued plus active plus `warm_buffer` (default 0), with no target tracking. |
| FL-3 | A task serving a session shall set ECS task scale-in protection and clear it when done (Q3). |

## Industry practice

| Pattern | Source | Here |
|---|---|---|
| A server-side agent loop with remote tool execution | Anthropic Managed Agents: the loop on the server, `EnvironmentWorker` runs tools and posts results | Followed, with the one files departure below. |
| A job queue in Postgres claimed with `FOR UPDATE SKIP LOCKED` | Postgres docs (`SELECT … FOR UPDATE SKIP LOCKED`); queues built on it: Que, GoodJob, Oban, pg-boss, River | Followed for the step queue, the work queue and schedules. |
| Durable execution: state in a log, each step replayable | Temporal (workflow history replayed by any worker), AWS Step Functions | Followed: the event log is the history; a step derives its action from it, so a rerun is safe. |
| A lease with an expiry instead of a held connection or lock | The SRE review of 2026-11-06; Temporal activity timeouts | Followed; a commit checks its claim is still its own. |
| Fan-out by signal, data from the source of truth | Common pub/sub notify-then-read designs | Followed with Valkey signals and log reads. |
| Context management in tiers: clear old tool output first, summarise later, keep the raw log | Anthropic context editing (C3) and compaction (C1); Codex (C5) | Followed (D31): the same 100,000-token clearing default and 3 kept results as context editing; summaries at 80% of the window with no fixed cap, below Codex's 90% (C5). |
| Credentials attached at egress, outside the sandbox | Live `cloud` (B19); Cloudflare outbound handlers (G5) | Followed for our `cloud` (D25, D38). |
| A per-session sandbox Durable Object with an outbound egress hook and R2 backups | Cloudflare's `claude-managed-agents` template (G9) | Followed and credited (D38); it differs where the template departs from live (CF-8 to CF-10). |
| Single-use workers scaled on queue depth | ECS target tracking with task scale-in protection (Q3) | Followed for the Fargate launcher and the loop workers (D33). |
| Envelope encryption for per-tenant secrets at scale | AWS KMS data keys (Q2) | The scale-up path behind `SecretStore`, with its trigger (D30). |

Departures from live: one.
- A session's work token reads its session's mounted files and saves its outputs through the existing files
  endpoints (FO-1, FO-2; decision D24). Live refuses this; Anthropic stages files inside its own `cloud` sandboxes.
  Why: agentd holds no container code (D5) and `agentd-worker` speaks only the public API (D6), so its launchers can
  stage a session's files only through the files endpoints.

Beside live, where agentd serves what live does not: models other than Anthropic's compact with a Codex-style handoff
prompt (CP-8) instead of Anthropic's compaction block, which only Anthropic's API returns. The Cloudflare swap's
5-minute value cache (CF-6) stays within live's "re-resolved periodically" (B12) and is recorded as an accepted risk
under SRE.

## Plan of PRs

One stack with `gh stack`: one spec PR, one cleanup PR (the 200-line rule), one production-safe migration PR, then
one PR per feature. Each PR deploys alone. Sessions on the loop (L7) sit behind a setting until Rollout turns them on, and
compaction (PR 7) runs only for those sessions and merges while L7 lists only the internal workspace (Rollout). Three PRs change what clients see on merge, each verified on stage first (Rollout): PR 4's refusal of
other events while a session waits on a tool (W-19) and its environment-key answers (W-15 to W-18, W-22, W-23), PR
5's overload refusal of new streams (F-5), and PR 8's vault and deployment checks (VA-2, VA-4, VA-5, MC-18, MC-19). The rest change no client-visible behaviour: PR 9's scheduler fires as today from any replica, and E-4 deletes, as
today, the session row and then its events through the port. Migration
numbers are taken at merge time. "Reuses" names work already written on local branches.

| # | PR | Kind | Parts | Requirements | Reuses |
|---|---|---|---|---|---|
| 1 | This spec | spec | P0 | all | |
| 2 | Make room: store settings, vault credentials and deployment run errors get their own files | cleanup | X1, X2, X3 | none (makes room) | |
| 3 | Migrations: `work_items`, `environment_keys`, `loop_steps`, `mcp_connections`, `schedules`, `webhook_deliveries`, the checkpoint and archived-session indexes | migration | W2m, W6m, Sm, MCm, Km, Hm, CPm, E4m | E-2, W-2, W-13, S-1, S-2, MC-13, K-1, H-4, CP-9 | |
| 4 | Work API for the official worker: work items in Postgres, live's work secret with `cloud` placeholders, environment keys and the runtime key, work-token scopes, files for workers | code | W1, W3 to W5, EV1, W7 to W9, L6, FO1 | W-1 to W-23, EV-1 to EV-4, L-9 to L-12, FO-1 to FO-3 | branches `environment-keys`, `harness-memory-stores`; PR #44's token scoping tests and files session paths |
| 5 | Event log and streams: ordered stamping, Valkey fan-out, the jittered fallback, overload refusal, the read replica, retention | code | F1, F2, F3, E4 | F-1 to F-5, E-2 to E-4 | `update_processed_at` (PR #44) |
| 6 | The loop: the engine restored, the step queue with claims and leases, the two roles, model limits and retries, the account-wide limiter, sessions on the loop behind a setting | code | L1 to L5, S1, S2, M1, L7 | S-1 to S-8, R-1, R-2, M-1 to M-4, L-1 to L-8, L-13 to L-19 | branch `self-hosted-work` |
| 7 | Compaction: model windows, both tiers, checkpoints | code | CP0 to CP3 | CP-1 to CP-13 | |
| 8 | MCP and vaults: the client with cached connections, networking checks, permission prompts, vault credentials read per step, the claimed OAuth refresh, deployment-fire checks, vault parity | code | MC1, MC2, MC3, VA1 | MC-1 to MC-27, VA-1 to VA-6 | |
| 9 | Scheduler for many replicas, and webhooks | code | K1, H1 | K-1, K-2, H-1 to H-6 | |
| 10 | Removals: the harness and its routes, agentd's sandbox launch, the egress crates | code | RM1, RM2, RM3 | ADR consequences, D25 | |
| 11 | Conformance, SLOs and docs: the Java SDK suite, SLO alerts, scale-up alarms, the load test, docs and runbook | code | J1, SRE | J-1 | |

11 agentd PRs including the spec. `agentd-worker` (its own repository, D6) builds section 11 in 3 PRs: the image with
the official `EnvironmentWorker`, the Docker launcher and file staging; the Cloudflare launcher (CF-1 to CF-12 with
EG-1, EG-3 to EG-5, EG-7, EG-8, EG-10); the optional Fargate and EC2 launchers with the AWS egress proxy and the fleet
(EG-2, EG-6, EG-9, FL-1 to FL-3).

### Per-PR detail

Each PR below is built from parts; each part lists its files, its statements per operation (sql-transactions rule 1), and its tests. A part's label (W1, MC2) names it wherever the spec refers to it. Database tests
are gated by `AGENTD_PG_TESTS`, as today.

#### PR 2. Make room

Every production file over 200 lines that a later PR changes is split here first, with no behaviour change: the standing 200-line rule wants this as a separate cleanup PR stacked before the change.

- **X1.** `config/store.rs` takes the store settings out of `api/src/config.rs`, as the 2026-11-06 plan has it.
- **X2.** `vaults/src/store.rs` splits its port in two: `VaultStore` (vault rows) and `VaultCredentialStore`
  (credential rows and their secrets), both implemented by the one Postgres store, so the vault aggregate keeps one
  repository. The credential operations move from `vaults/src/service.rs` (304 lines) to `vaults/src/credentials.rs`
  and from `store-postgres/src/entities/vaults.rs` (295 lines) to `entities/vault_credentials.rs`; every file ends
  under 200 lines. No statement changes. Tests: the existing vault and credential suites, unchanged, pass.
- **X3.** `deployments/src/run_error.rs` (new) takes `RunError`, `RunErrorKind`, `PausedReason` and
  `classify_fire_error` from `types.rs` (318 lines) and `service.rs` (308 lines), and the after-fire bookkeeping
  (auto-archive on `agent_archived`) from `service.rs` as `apply_run_error(&mut Deployment, &RunError)`. No
  behaviour change; the deployment suites pass unchanged.

#### PR 3. Migrations

Every new table and index, production-safe: new tables only, unused until their PR's code ships; indexes on existing tables are built `CONCURRENTLY` in their own migration files.

- **W2m.** `create table work_items (workspace_id, session_id, org_id, environment_id, queue, state, metadata,
  created_at, acknowledged_at, started_at, latest_heartbeat_at, last_heartbeat, lease_until, handed_out_at,
  stop_requested_at, stopped_at, actor, request_id, primary key (workspace_id, session_id))`; indexes
  `(queue, created_at) where state = 'queued'` and `(workspace_id, environment_id, created_at desc)`. `actor` and
  `request_id` default from the connection settings, as every table since history PR #41. Also the index
  `(environment_id) where queue = 'runtime'` for W9.
- **W6m.** `create table environment_keys (environment_key_id, org_id, workspace_id, environment_id, name,
  key_digest unique, partial_key_hint, created_at, revoked_at)`.
- **Sm.** `create table loop_steps (workspace_id, session_id, due_at, wakes, lease_owner, lease_until, attempts,
  primary key (workspace_id, session_id)) with (fillfactor = 70, autovacuum_vacuum_scale_factor = 0.01,
  autovacuum_analyze_scale_factor = 0.02, autovacuum_vacuum_cost_limit = 2000)`, since every step updates and
  deletes its row; partial index on `due_at` where `lease_until is null`; index on `lease_until`.
- **E4m.** In its own file: `create index concurrently sessions_archived_at on sessions ((doc->>'archived_at'),
  session_id) where doc->>'archived_at' is not null`, for E4's walk.
- **Km.** `create table schedules (target, workspace_id, spec, next_fire_at, paused, primary key (target))`; index on
  `next_fire_at` where not paused.
- **Hm.** `webhook_deliveries` (the configured endpoint's key and URL, body, attempt, `next_attempt_at`,
  `claimed_until`, `delivered_at`, `dropped_at`).
- **MCm.** `create table mcp_connections (workspace_id, session_id, server_url, mcp_session_id, tools jsonb,
  credential_updated_at, connected_at, actor, request_id, primary key (workspace_id, session_id, server_url))`.
- **CPm.** `create unique index concurrently session_events_checkpoint on session_events (workspace_id, session_id,
  (doc->>'session_thread_id'), ((doc->'checkpoint'->>'through_seq')::bigint)) where doc->>'type' =
  'agent.thread_context_compacted'`.

#### PR 4. Work API for the official worker

- **W1.** `secrets-aws/src/system_secret.rs` (new, about 55 lines) holds the read-or-create body that `pepper.rs`
  has today; `work_token_key()` and `api_key_pepper()` call it. Boot: 1 `GetSecretValue`, 1 `CreateSecret` the first
  time; per request none. Tests (LocalStack): the second read returns the first's bytes; a token minted under one
  boot verifies under the next; with `AGENTD_WORK_TOKEN_KEY` set, tokens are signed with it and Secrets Manager is
  not read (W-20).
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
  queued; an archived session's item reads `stopping`; a heartbeat with no TTL is granted 300 s (W-7), with
  `desired_ttl_seconds: 45` is granted and echoed as 45 (W-21), and with `"x"` gets live's 400 (W-8); an
  `active` item with no heartbeats still reads `active` (W-10).
- **W5.** `work/src/tokens.rs` `secret()` writes live's nine keys in live's order. Test: the decoded secret's keys,
  order and types.
- **EV1.** `work/src/secret.rs` (new) builds `environment_variables` for `cloud` sessions and an empty object for
  `self_hosted` ones; `work/src/placeholder.rs` (new) signs placeholders with `Signer` keyed by
  `<prefix>/system/egress_key`, read or created through W1's `system_secret.rs`; `secrets-aws/src/name.rs` adds the
  `credential_type` tag, which `entities/vault_credentials.rs` passes for `environment_variable` credentials;
  `api/src/cli/vault_secrets.rs` (new) is `agentd vault-secrets tag`. Statements per `cloud` hand-out: 1 SELECT of the session by primary key (its `vault_ids`); 1 `SELECT *
  FROM credentials WHERE workspace_id = $1 AND vault_id = ANY($vault_ids) AND doc->'auth'->>'type' =
  'environment_variable' AND doc->>'archived_at' IS NULL`; no secret read; none for `self_hosted`. The key is read
  once per process. The tag command: 1 SELECT of `environment_variable` credentials, then 1 `TagResource` each.
  Tests: the decoded secret maps each name to a placeholder that verifies for that session and fails for another;
  the first vault in `vault_ids` wins a shared name; an archived credential is left out; a `self_hosted` session's
  secret has `"environment_variables": {}`; a new `environment_variable` secret carries the tag and an MCP one does
  not (LocalStack); no credential value appears in the secret, the events or the captured logs.
- **W7.** `api-keys/src/environment.rs`, `store-postgres/src/entities/environment_keys.rs`,
  `api/src/cli/environment_keys.rs`, `api/src/edge/reach.rs` (what each scoped credential reaches),
  `edge/auth.rs` (resolution only), `work/src/routes.rs` (poll). Per request 1 SELECT by digest, as for workspace
  keys. Tests (`api/tests/edge.rs`, `api/tests/environment_keys.rs`): one per W-13 to W-18 with live's bodies.
- **W8.** `sessions/src/service/events.rs`: the session row is already read; no new statement. Tests: refused with
  live's text; the four answers accepted.
- **W9.** `api/src/edge/reach.rs` (W7) allows the runtime key on `GET /v1/environments/{id}`; the environments route
  checks it with 1 `SELECT workspace_id FROM work_items WHERE environment_id = $1 AND queue = 'runtime' LIMIT 1`, then
  serves the environment from that workspace as today (1 SELECT); W2m adds the index `(environment_id) where queue =
  'runtime'`. Tests (`api/tests/environment_keys.rs`): the runtime key reads an environment with a `cloud` session;
  it gets live's 403 for an environment with none; a customer's environment key gets live's 403 for its own
  environment.
- **L6.** `api/src/edge/reach.rs` (token routes); the memory routes' guard asks sessions which stores a session
  attaches and how; every token request reads its item (1 SELECT by primary key). Tests: one per L-9 to L-12.
- **FO1.** `files/src/session.rs` (output upload, delete, download of mounted files) asking `Session::mounts`.
  Download: 1 SELECT of the session, 1 SELECT of the file, 1 object GET. The grader reads a rubric file in the loop,
  server side, so no worker downloads it. Tests (`api/tests/session_token.rs`): its own mounted file is served;
  another session's file is refused; a file not mounted in its session is refused; an upload is scoped to its
  session and absent from another session's `files.list(scope_id=...)`; deleting another session's file or a
  mounted input answers 404 (FO-3).

#### PR 5. Event log and streams

- **F1.** `entities/session_events.rs`: append is one multi-row INSERT; `update_processed_at(ws, session,
  ids, at)` is one `UPDATE … SET doc = jsonb_set(doc, '{processed_at}', …) WHERE … AND event_id = ANY($ids) AND
  doc->>'processed_at' IS NULL RETURNING seq, doc`, so a step stamps exactly the events it read. Every append
  first takes `pg_advisory_xact_lock(hashtext(session))`, so a session's `seq` order is its commit order and a
  keyset read on `seq` never skips an event that commits late. No migration: `processed_at` stays in `doc`, as
  today. Tests: two stamps get disjoint events; a stamp leaves an unread event and a status event unstamped;
  concurrent appends to one session are read back in commit order, each once.
- **F2.** `sessions/src/service/events.rs` and a `signals` adapter over Valkey; per replica one log read per tick
  for every signalled session it streams. Tests: an event appended on one replica reaches another's stream in
  order; a dropped signal is recovered by the fallback timer; delete ends the other replica's stream.
- **F3.** `sessions/src/service/stream.rs`: the fallback timer of 5 s plus up to 1 s of jitter; the overload gate
  reads the pool's wait time and a 10 s cached `SELECT count(*) FROM pg_stat_activity` against `max_connections`;
  `store-postgres/src/pool.rs` opens the replica pool when `AGENTD_PG_READ_URL` is set and `events.list` uses it.
  Tests: with signals off, an event reaches the stream within 6 s; with the gate forced, a new stream gets 529
  `overloaded_error` and an open one keeps streaming; with a replica URL set, `events.list` reads it and a step
  reads the primary.
- **E4.** Every event-log access stays behind `SessionEventLog` (D18), and one method deletes events:
  `SessionEventLog::delete_events_of(&[(workspace, session)])`, 1 `DELETE FROM session_events USING unnest($ws, $ids)
  AS v(workspace_id, session_id) WHERE session_events.workspace_id = v.workspace_id AND session_events.session_id =
  v.session_id`; today's `delete_events` goes. Retention, `sessions/src/retention.rs` (new): every 5 minutes one
  replica wins `pg_try_advisory_lock(hashtext('event_retention'))` and holds that one pooled connection for the
  run, ending with `pg_advisory_unlock(hashtext('event_retention'))` on it; on any other exit, an error or a panic,
  the run detaches that connection from the pool and closes it, which frees the lock. A process's first run walks every archived
  session older than `$cutoff`, from the start of E4m's index; later runs walk `$cutoff - 1 hour` to `$cutoff`. Either walk pages
  by `(archived_at, session_id)`, 500 sessions a page: `SessionsStore::archived_between` issues 1 `SELECT
  workspace_id, session_id, doc->>'archived_at' FROM sessions WHERE (doc->>'archived_at', session_id) > ($after,
  $after_id) AND doc->>'archived_at' < $to ORDER BY doc->>'archived_at', session_id LIMIT 500`, then 1
  `delete_events_of` for that page, until a page comes back short. The read crosses from the sessions store to the
  event log's port, which sql-transactions rule 7a's single statement cannot do without tying the log to Postgres:
  the user's port rule (D18) wins. Each DELETE covers at most 500 sessions. `agentd_retention_last_success` is a
  metric; PR 11 builds its alarm (`agentd-retention-stale`, 45 minutes without a run, inside the 1-hour horizon) and
  the runbook action for a longer outage: restart one replica, whose first run walks from the oldest. Its test: the
  metric advances after a run. Session delete, in
  `sessions/src/service/lifecycle.rs` and `store-postgres/src/entities/sessions.rs`: `SessionsStore::delete` is one
  transaction: 1 `SELECT pg_advisory_xact_lock(hashtext($session))`, the lock every append takes (F1) and, from PR 6, every step commit, so an append in flight finishes first and a later one finds no session; 1 `DELETE FROM sessions WHERE
  workspace_id = $1 AND session_id = $2 AND status <> 'running' RETURNING doc`, and on 0 rows 1 `SELECT status FROM
  sessions WHERE …` on the failure path to answer 404 or today's `cannot delete a running session` error; from PR 6, 1 `DELETE
  FROM loop_steps WHERE workspace_id = $1 AND session_id = $2`, so a queued step is gone and a running one finds its
  claim gone at commit and rolls back (S-4); from PR 4, W3's stop (W-11: 1 UPDATE); from PR 8, MC1's `mcp_connections` delete
  (MC-22: 1 `DELETE FROM mcp_connections WHERE workspace_id = $1 AND session_id = $2`). `SessionsStore::delete` calls
  those two methods with its transaction, so each has one version, taking the caller's connection. Today's pre-read of the session goes.
  After the commit, 1 `delete_events_of` with the one session (E-4), outside the transaction because the log sits
  behind its port; a crash between the two leaves events no API can reach (an accepted risk under SRE). Tests: with a
  period of 1 day, an archived session two days old loses its events and one archived today keeps them; turning the
  setting on purges sessions archived 30 days ago; a run that fails mid-page leaves the lock free for the next run; a run skipped for 40 minutes is caught by the next; two replicas
  never run retention at once; with no period set, nothing is deleted; a deleted session's events are gone; a
  `user.message` racing a delete has one of three outcomes, each asserted in a looped test: the session runs and the
  delete answers 409; the message is refused because the session is gone; or the message is accepted, then the
  delete succeeds and the work item reads stopped. S1's test in PR 6 adds that no step runs after the delete.

#### PR 6. The loop

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
  step; deleting a session during a running step makes that step's commit roll back, and no step runs after the
  delete.
- **S2.** `crates/engine/src/worker.rs` (claim, run one step, commit), `api/src/config/role.rs` (`AGENTD_ROLE`,
  `AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE`, `AGENTD_MODEL_ATTEMPTS`, `AGENTD_STEPS_PER_WORKER`,
  `AGENTD_PG_POOL_SIZE`); the two gauges come from 1 `SELECT count(*) FILTER (WHERE lease_until IS NULL AND due_at <=
  now()), count(*) FILTER (WHERE lease_until > now()) FROM loop_steps` every 10 s per replica. Tests: an `api`
  replica claims no step; two `loop` replicas share the queue; a workspace at its limit leaves its next step for
  later and others run; a worker set to 4 steps runs no fifth at once; the gauges match the table.
- **M1.** `engine/src/model/budget.rs` (new): per provider and minute, one Valkey `INCRBY` of requests and of the
  estimated input tokens (CP-1) on a key that expires after 120 s; over the budget, the step is deferred with the
  statement in the step queue table and no model call. Tests: with a budget of 2 requests a minute, a third step is
  deferred to the next minute and runs then; Valkey down fails the step's claim back to the queue, not open.
- **L7.** `sessions/src/service/sandbox.rs`: a session launches nothing; its work is queued at create on its
  environment or the runtime queue; the flag lists workspaces. An idle session created on the harness path in a
  listed workspace runs its next turn on the loop (L-19): its event log is its whole state (D8), so the loop reads it
  as any other. Tests: the live poll-path cases replayed with `ant` pollers and the scripted model; a session started
  on the harness, left idle, then woken after its workspace is listed, runs its turn on the loop with its history.

#### PR 7. Compaction

- **CP0.** `shared/src/limits/models.rs` (69 lines) holds each model's window beside its id; `engine/src/model/
  windows.rs` (new) reads `GET /v1/models/{id}` once per process per model, and `AGENTD_MODEL_WINDOWS`
  (`model=tokens,...`) for other providers. Tests: a scripted models endpoint answering `max_input_tokens: 200000`
  sets that window; a provider with no endpoint takes the setting; a model with neither refuses to start a step with
  a clear log line.
- **CP1.** `engine/src/turn/compact/` (new, each file under 200 lines): `estimate.rs` (CP-1 from the last
  `span.model_request_end` usage and the bytes since), `clear.rs` (CP-2 placeholders `[tool result cleared: event
  <id>]`), `cap.rs` (CP-3). Prompt building only; no statement. Tests (thresholds set to 15% of a scripted model's
  window): past it, results older than the last 3 are placeholders naming their event ids, inputs and error results
  stay; below it, nothing changes; a 50,000-token result reaches the prompt cut at the limit and the stored event is
  whole.
- **CP2.** `engine/src/turn/compact/step.rs` and `summary.rs`: the step chooses compaction when CP-4's threshold is
  passed and CP-5 holds; the summary call reuses the turn's system prompt and tools (so the prompt cache hits) with
  `compact-2026-09-04`; the checkpoint event's stored `checkpoint` object holds the block, `through_seq` and the
  model, and the API's typed event drops it on read. Statements: the step's commit (step queue table) appends the
  checkpoint in its multi-row INSERT; the step queue's Run read (`SELECT DISTINCT ON (doc->>'session_thread_id') …`, CPm's index) finds each
  thread's latest checkpoint; 1 keyset SELECT
  of the events after it. Tests (a scripted model with a 20,000-token
  window, so 80% is reached in a few turns): a long scripted conversation compacts once, writes one `agent.thread_context_compacted` with live's
  fields only on list and stream, and the next call sends the block first and unmodified; a fact planted in an early
  user message is in the first prompt after compaction (verbatim or in the summary); a second compaction starts from
  the first checkpoint; a step with a tool call waiting does not compact; a summary call that returns no block
  leaves the cleared prompt and compacts at the next step; every call after compaction sends the
  `compact-2026-09-04` beta header with the block (CP-7). A recall check against a real model runs only with the
  user's OK (D22).
- **CP3.** `engine/src/turn/compact/handoff.rs` (the handoff prompt), `oversize.rs`, and the model-change check in
  `step.rs`. Tests: an OpenAI-scripted thread compacts with the handoff prompt and starts its next prompt with the
  summary; a compaction request over the window drops its oldest events and succeeds; switching to a model with a
  smaller window, or reading a checkpoint written by another model, compacts again before the next turn.

#### PR 8. MCP and vaults

- **MC1.** `crates/engine/src/mcp/` (the MCP client over streamable HTTP) and
  `store-postgres/src/entities/mcp_connections.rs`. The step commit gains the MCP branch: the step queue port's
  commit (`sessions/src/steps.rs`) takes a `keep_due` flag, which `crates/engine/src/worker.rs` sets when the step
  leaves an allowed `agent.mcp_tool_use` unanswered or ran MCP calls with no model call (MC-25), and
  `store-postgres/src/entities/loop_steps.rs` then issues the
  release UPDATE with no DELETE (the step queue table's Commit row). Per step: 1 `SELECT * FROM mcp_connections WHERE workspace_id =
  $1 AND session_id = $2`; a reconnect (MC-15) sends `initialize` and `tools/list` and writes 1 `INSERT … ON CONFLICT
  (workspace_id, session_id, server_url) DO UPDATE`; a `notifications/tools/list_changed` adds `tools/list` and the
  same upsert; the session's delete commit adds 1 `DELETE FROM mcp_connections WHERE workspace_id = $1 AND session_id
  = $2`. Calls run with the step's deadline. An MCP tool call runs in the loop: no work item, no worker. The
  permission evaluation follows the agent's `mcp_toolset` configs; an `ask` idles the session as a built-in
  `always_ask` call does. Before connecting, the loop checks the environment's networking through
  `environments/src/egress.rs` (new, shared with MC3) and the resolved address. Tests against a scripted MCP server:
  tools listed and called, with `agent.mcp_tool_use` (`evaluated_permission` `allow`) and `agent.mcp_tool_result`;
  no work item is queued for an MCP call; a step commits `agent.mcp_tool_use`, leaves its `loop_steps` row due at once, and the next step calls the tool
  and commits its result with no model call, then the step after calls the model (MC-25); a server redirecting to
  `127.0.0.1` is refused (MC-11);
  a server that drops after the choosing step gets one `is_error` `agent.mcp_tool_result` (MC-26) and one
  `mcp_connection_failed_error` `session.error` (MC-5), and that step's `loop_steps` row is deleted, not kept due;
  with two allowed calls to one server in a step and the first failing, the second gets an `is_error` result with no
  new connection attempt (MC-14, MC-26), and the step records as many `session.error` events as B27's capture shows;
  the `is_error` content matches B27's capture; a
  deferred MCP call (MC-20) makes exactly one model call in all; a second step sends no `initialize` and reuses the stored session id; a 404
  to it, a changed credential `updated_at` or an idle-to-running transition reconnects; a `list_changed`
  notification refreshes the stored list; one session never reads another's row; deleting a session deletes its rows in that commit; an `always_ask` tool idles
  `requires_action`, runs after an allow and records an `is_error` result after a deny; a `limited` environment
  without `allow_mcp_servers` and with the host not listed records `mcp_connection_failed_error` without connecting;
  a URL resolving to `127.0.0.1` or `169.254.169.254` is refused; an unreachable server records
  `mcp_connection_failed_error`, the session continues, the same turn does not retry, and the next idle-to-running
  transition does.
- **MC2.** `crates/engine/src/mcp/credentials.rs`; `vaults/src/refresh.rs` (new), the loop's entry into the vaults
  service: `VaultService::refresh_mcp_oauth(&WorkspaceId, &VaultId, &CredentialId)`, which takes the session's
  workspace from the step, not an API `AuthContext`; `VaultCredentialStore::replace_credential(ws, vault, cred, guard, secret)` takes
  an optional `updated_at` guard and a `SecretWrite` (`Keep`, `Replace` or `Purge`) and answers whether a row matched;
  `Replace` writes the secret before the row, so a crash between them leaves a fresher token under an old expiry and
  costs one more refresh; `Purge` deletes the secret after the row, as archiving does today; the API's update passes
  no guard and `Replace`, archiving no guard and `Purge`. One method, in `entities/vault_credentials.rs`. Statements
  per step: 1 `SELECT * FROM credentials WHERE workspace_id = $1 AND
  vault_id = ANY($vault_ids) AND doc->'auth'->>'mcp_server_url' = ANY($urls) AND doc->>'archived_at' IS NULL` for all
  the agent's servers (an archived vault's credentials are archived with it, VA-3); then, for each server the step connects to or
  calls, 1 `GetSecretValue` through `SecretStore`. A refresh, in order, when the service sees the row's `updated_at`
  is more than 30 s old (when it is newer, another worker holds the refresh: the step ends and runs again 1 s later, MC-20):
  the claim, `replace_credential` with the guard `$read` and `Keep` (1 `UPDATE credentials SET
  display_name = $2, has_secret = $3, doc = $4 WHERE workspace_id = $5 AND vault_id = $6 AND credential_id = $7 AND
  doc->>'updated_at' = $read`, the new `doc` carrying `updated_at = $now`); then the POST to `token_endpoint` (10 s
  timeout); then `replace_credential` with the guard `$now` and `Replace` (1 `PutSecretValue`, then the same UPDATE
  recording the new `expires_at`). When that last UPDATE matches no row, an API update of the credential came between
  the claim and the write: the refresh's token is in Secrets Manager under the API's row, the loop logs
  `refresh_overwrote_update` with the credential id, and the user's next update applies (an accepted risk under
  SRE). A claim that matches no row (MC-23) re-reads with
  `get_credential` (1 SELECT by primary key, 1 `GetSecretValue`) and uses the token when unexpired; otherwise the
  step ends with its row due again in 1 s. A failed refresh records `mcp_authentication_failed_error`; H1 (PR 9)
  adds the `vault_credential.refresh_failed` delivery. Tests: an expired token is refreshed, stored, its new expiry
  shown by `credentials.retrieve`, and used on the next call; two workers refreshing at once make one request to the
  token endpoint and the loser uses the stored token; a loser that finds the stored token still expired ends its step,
  which runs again 1 s later; a worker that reads an expired row claimed less than 30 s ago ends its step with no further read and runs 1 s later; an API update
  between a claim and its write logs `refresh_overwrote_update` and keeps the API's row; a worker killed after its claim lets another refresh after
  30 s; a refresh refused by the token endpoint records `mcp_authentication_failed_error`; a `token_endpoint` resolving to
  `127.0.0.1`, or redirecting to `169.254.169.254`, is not called and the refresh fails (MC-27); the credential comes from
  the first of the session's vaults that holds one, and none after that vault is archived; with none, the
  connection is unauthenticated and a refusing server records `mcp_authentication_failed_error`; a step that neither
  connects nor calls reads no secret; a rotated secret is used on the next call and an archived credential is not;
  no secret appears in any event, log line or output (a test sink scans them all).
- **MC3.** `sessions/src/service/lifecycle.rs` (162 lines): a create with `deployment_id` set checks the agent's MCP
  hosts against the environment's networking with `environments/src/egress.rs`, on the agent and environment rows
  create already reads, and refuses with a typed `mcp_egress_blocked` error; `run_error.rs` maps it to
  `RunErrorKind::McpEgressBlocked` (`mcp_egress_blocked_error`) and `apply_run_error` sets
  `PausedReason::Error { error }`. No new statement: the run row and the deployment row are written as for any
  failed fire. Tests: a deployment whose agent's MCP host is not allowed records a run with
  `mcp_egress_blocked_error`, creates no session, and reads back paused with live's `paused_reason`; with
  `allow_mcp_servers` true or the host listed, the fire creates the session; a session created directly (no
  deployment) is not checked at create.
- **VA1.** `sessions/src/service/lifecycle.rs`: an update with `vault_ids` keeps today's refusal with live's message;
  a create reads its vaults (1 `SELECT * FROM vaults WHERE workspace_id = $1 AND vault_id = ANY($ids)`) and refuses
  an archived or missing one. `vaults/src/rotation.rs` (new) takes the update rules out of `credentials.rs` and adds
  the fixed `token_endpoint` and the kept expiry, so `credentials.rs` stays under 200 lines. `vaults/src/validate.rs`
  (new) and a `mcp_oauth_validate` route in `routes.rs` (136 lines): 1 SELECT of the credential, 1 `GetSecretValue`,
  and `initialize` through an `McpProbe` port that the engine's MCP client implements, wired in `api`. Live's
  statuses and messages for VA-1, VA-2 and VA-4 are captured with a probe that makes no model call, and the tests
  assert them. A vault's archive, in `vaults/src/service.rs`, is one transaction: 1 `UPDATE vaults` (as today) and 1
  `UPDATE credentials SET has_secret = false, doc = jsonb_set(jsonb_set(doc, '{archived_at}', $now), '{updated_at}',
  $now) WHERE workspace_id = $1 AND vault_id = $2 AND doc->>'archived_at' IS NULL RETURNING credential_id`; after the
  commit, 1 secret delete per returned credential, as a credential's archive purges today. Tests: each of VA-1, VA-2,
  VA-3, VA-4 and VA-5, VA-3 with a running session that keeps running and gets no credential from the archived vault
  at its next step; `mcp_oauth_validate` answers `valid` against a scripted
  server that accepts the token, `invalid` on 401, `unknown` when unreachable.

#### PR 9. Scheduler and webhooks

- **K1.** `crates/adapters/scheduler-postgres` replaces `scheduler-local` in `backends.rs`. Tick, per replica every
  10 s, one transaction: 1 `SELECT … WHERE next_fire_at <= now() AND NOT paused FOR UPDATE SKIP LOCKED LIMIT 100`;
  1 `UPDATE schedules SET next_fire_at = v.next FROM unnest($targets, $nexts) AS v(target, next) WHERE
  schedules.target = v.target` (no backfill); then the fires, outside it; `claim_fire` stays the
  exactly-once guard. Tests: two schedulers on one database fire each occurrence once; a schedule registered on one
  fires on the other.
- **H1.** `crates/resources/webhooks`: endpoints read from configuration (D20); one multi-row INSERT of deliveries
  per status change in its commit; a task per replica claims due rows 100 at a time `FOR UPDATE SKIP LOCKED`, signs,
  POSTs with a 10 s timeout, and marks them in one UPDATE per batch. Tests: signature verified with the SDK's
  `client.beta.webhooks.unwrap()`; a 401 receiver sees three attempts with one `webhook-id`; a due delivery
  survives a restart; a failed OAuth refresh sends `vault_credential.refresh_failed`; archiving a vault with two
  credentials sends two `vault_credential.archived` deliveries (H-6).

#### PR 10. Removals

Merges after the loop's setting reaches every workspace (Rollout).

- **RM1.** Removes `harness/`, `api/src/harness.rs`, `HarnessContract` writes used only by the harness, the harness
  CI job. CI's `sdk` and `demo` jobs run agentd's loop with the scripted model and the container project's worker.
- **RM2, RM3.** Remove the crates and wiring named in the table once L7 is on everywhere; they move to the container
  project (Out of scope).

#### PR 11. Conformance, SLOs and docs

- **J1.** `conformance/java` (Gradle, `com.anthropic:anthropic-java` 2.52.0 from the version catalog): the Python
  suite's cases against agentd with only the base URL changed, run in the `sdk` CI job after the TypeScript suite.
  Tests: the suite itself, green in CI.
- **SRE.** `docker/observability/slos.yaml` gains the alert rules for the five SLOs, and `docker/observability/alarms/`
  the five scale-up alarms (SRE, scale-up triggers) and `agentd-retention-stale` (E4) with their metrics and
  thresholds; `perf/` gains the load test
  runs; the docs and the runbook (DOC1) describe the roles, the settings, the launch deployment and each alarm's
  action. Tests: the alert rules load in the local observability stack; the load test's steady run passes in CI's
  `perf-smoke` job.

### The step queue: statements per operation (S1, S2)

| Operation | Statements |
|---|---|
| Enqueue, in the commit that appends the waking event (after its session append lock) | 1 `INSERT INTO loop_steps … ON CONFLICT (workspace_id, session_id) DO UPDATE SET wakes = loop_steps.wakes + 1` |
| Claim, one transaction | 1 `SELECT … WHERE due_at <= now() AND (lease_until IS NULL OR lease_until < now()) ORDER BY due_at FOR UPDATE SKIP LOCKED LIMIT 1`; 1 `SELECT pg_try_advisory_xact_lock(hashtext($workspace))`, and when it returns false the claim ends, so no claimer waits on the lock while holding a row; 1 `SELECT count(*) FROM loop_steps WHERE workspace_id = $1 AND lease_until > now()`; at the limit, 1 `UPDATE loop_steps SET due_at = now() + interval '1 second' WHERE workspace_id = $1 AND lease_until IS NULL` defers the whole workspace and the claim ends; under it, 1 `UPDATE … SET lease_owner = $me, lease_until = now() + interval '12 minutes', attempts = attempts + 1` |
| Run, no transaction open | 1 `SELECT DISTINCT ON (doc->>'session_thread_id') seq, doc FROM session_events WHERE workspace_id = $1 AND session_id = $2 AND doc->>'type' = 'agent.thread_context_compacted' ORDER BY doc->>'session_thread_id', (doc->'checkpoint'->>'through_seq')::bigint DESC` for every thread's latest checkpoint (CPm's index); 1 keyset SELECT of the session's events after the oldest of those checkpoints, or from `seq` 0 when a thread the step builds has none, noting the ids of the client events it read; the model call |
| Commit, one transaction | 1 `SELECT pg_advisory_xact_lock(hashtext($session))` first, the same lock every append of the session takes before touching `loop_steps`, so the two never wait on each other in a cycle; when the step leaves an allowed `agent.mcp_tool_use` unanswered (MC-24, from PR 8), 1 `UPDATE … SET lease_owner = NULL, lease_until = NULL, due_at = now() WHERE … AND lease_owner = $me AND attempts = $claimed` and no DELETE; otherwise 1 `DELETE FROM loop_steps WHERE … AND lease_owner = $me AND attempts = $claimed AND wakes = $claimed_wakes`, and when it deletes nothing, the same UPDATE; when that UPDATE updates nothing, the claim was taken over: roll back; 1 `UPDATE session_events SET doc = jsonb_set(doc, '{processed_at}', …) WHERE … AND event_id = ANY($read_ids) AND doc->>'processed_at' IS NULL`; 1 multi-row INSERT of the step's events; the session row writes those events cause, as `record` does today; when the step changes the session's status or a refresh failed, 1 multi-row `INSERT INTO webhook_deliveries` (H1) |
| Defer at the account-wide model budget (M-4), a credential claimed by another worker (MC-20), or a lost refresh claim whose stored token is still expired (MC-23) | 1 `UPDATE loop_steps SET lease_owner = NULL, lease_until = NULL, due_at = $when WHERE … AND lease_owner = $me AND attempts = $claimed` |
| Compaction step (CP2) | the Run and Commit statements; the Commit's multi-row INSERT carries the checkpoint |
| Takeover | the Claim statements: a lease that has run out is claimable |

## Rollout

- The work API (PR 4) ships without a flag. Its work routes change only agentd's own harness work, since no client
  polls work on `main`; what clients see changes in two places, each verified on stage with a probe matching live's
  captured answer before prod: a session idle with `requires_action` refuses other events with live's 400 (W-19),
  and environment keys get live's answers (401 for W-15 and W-18, 403 for W-16 and W-23, 404 for W-17, the
  environment for W-22).
- PR 5's overload refusal (F-5) is verified on stage with the gate forced: a new stream gets 529 `overloaded_error`
  and open streams keep streaming; prod follows after a day with no page.
- PR 8's checks are verified on stage with a probe of each refused case (VA-2, VA-4, VA-5) and a deployment whose
  MCP host the environment blocks (MC-18, MC-19), each matching live's captured answer, and with the Python,
  TypeScript suites green (the Java suite joins with PR 11); prod follows after a day with no page.
- Sessions on the loop (L7, in PR 6) sit behind a setting listing workspaces; a session's path is fixed at create, except that an idle harness-path session moves to the loop at its next turn
  once its workspace is listed (L-19). Stages: one internal
  workspace, then 10%, 50%, 100% of workspaces, each for at least 24 h including the daily peak, judged on the SLIs
  below, with an absolute stop when a page fires.
- A workspace is listed for L7 only once `agentd-worker` polls the runtime queue in that deployment environment;
  until then its `cloud` sessions would wait on a queue nobody polls.
- The removals (PR 10) merge after L7 reaches 100%, `agentd-worker` serves `cloud` work on stage and prod, and no
  session is running on the harness path: idle ones move to the loop at their next turn (L-19), and the merge waits
  until a count of running harness-path sessions reads zero on stage and prod.
- Compaction (PR 7) runs only for sessions on the loop, and PR 7 merges while L7 lists only the internal
  workspace, so compaction reaches other workspaces only through L7's later stages.
- The Cloudflare launcher serves stage's runtime queue first; prod's runtime queue moves to it after a week on stage
  with no page, and after Cloudflare has raised the account's vCPU capacity (G1) above the expected peak.
- Per deployment environment, the settings (`AGENTD_ROLE`, model limits, the L7 workspace list) live in its
  configuration.

## Out of scope

- The DynamoDB event log (D18), the scale-up step for large workspaces, planned when `agentd-event-log-hot` fires.
  The event log stays behind its port; the switch replaces the adapter behind it and the two Postgres guarantees named
  below. Its requirement for that plan: where a
  workspace is listed for DynamoDB, the API shall keep its new sessions' events in DynamoDB behind the same port,
  with the same order, take and read semantics, and keep each session in the store it was created in. That plan
  also solves what one Postgres commit gives today: a step's claim release, stamps and appended events together
  (S-3); and it moves retention (E-2) to DynamoDB's own expiry, set when a session is archived.
- Envelope encryption (D30): the scale-up path for credential storage, behind `SecretStore` in agentd and the swap's
  credential source in `agentd-worker`, for MCP and `environment_variable` credentials together. It is planned when
  its trigger fires (SRE, scale-up triggers).
- Tunnels and user profiles (D37, B25): each gets its own parity plan.
- **`agentd-worker`**, the container project (its own repository, D6), built to section 11: the image running the
  official `EnvironmentWorker` unchanged; the Docker launcher for development and CI, the Cloudflare launcher for our
  `cloud`, and the optional Fargate (with its fleet) and EC2 launchers; staging files in and outputs out through the
  public files endpoints with the work token (FO-1, FO-2); `/workspace` snapshots; the swap (the Cloudflare Durable
  Object's handler, or the optional AWS egress proxy) with networking enforced by default and secret injection on in
  our `cloud` deployment and off by default elsewhere (D25); our runtime poller and its `--on-work` command. It
  speaks only the public API, so it runs against live as well. Its plan is a separate document. `sandbox-docker`,
  `sandbox-worker` and the egress crates move there (RM2, RM3). (The relay binary in `sandbox-worker`, also named
  `agentd-worker`, moves with it.)
- Session webhook registration through an API: live has none (B8).

## Deployment

The launch deployment (D34) and the scale-up path, each switch a setting:

| Part | Launch | Scale-up, and its switch |
|---|---|---|
| agentd | one container, `AGENTD_ROLE=all`, on tosspaper's existing host or ECS, behind its load balancer and NAT | separate `api` and `loop` services (`AGENTD_ROLE`), loop workers autoscaled on `agentd_loop_steps_due` |
| Postgres | its own `agentd` schema in tosspaper's Postgres, a pool of 16 per replica | its own RDS instance (`AGENTD_PG_URL`), then RDS Proxy in front (the same setting), then a read replica (`AGENTD_PG_READ_URL`) |
| Pub/sub | tosspaper's Valkey | its own Valkey (`AGENTD_VALKEY_URL`) |
| Objects, logs, metrics | tosspaper's S3 bucket, logs and metrics | unchanged |
| Event log | Postgres, behind the event-log port | DynamoDB for large workspaces behind the same port, a later plan (Out of scope) |
| Credentials | Secrets Manager (D30) | envelope encryption, a later plan |
| `cloud` sandboxes | Cloudflare Containers, no warm pool; our runtime poller on tosspaper's host | the same, with a raised account capacity; Fargate with its fleet and `warm_buffer` for an AWS-only operator |

Cost at launch for 10,000 extractions a month, excluding the model (G2): agentd, Postgres, Valkey and S3 run on
what tosspaper already pays for. Cloudflare: the $5 Workers plan; containers from about $23 (`basic`, 5 minutes of
full CPU per extraction) to about $93 (`standard-1`, 10 minutes at half CPU), after the plan's included usage;
Durable Objects under $5. AWS data transfer for staging documents from S3 to Cloudflare, about $15 to $20 for some
200 GB. Secrets Manager, $0.40 a credential a month. In all about $45 to $125 a month.

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
| Step statements | about 11 per step (the checkpoint read included), plus 1 when the step changes the session's status (a webhook delivery INSERT) | about 5×10^3/s on Postgres |
| A workspace at its limit | 4 statements per claim that finds it at the limit; a step enqueued during the deferral can cause one more | bounded by the enqueue rate of workspaces at their limit plus one deferral per second each |
| Log reads | 500 steps/s × the log size a step reads: from the oldest checkpoint of the session's threads, or the whole log while a thread has none, so compaction bounds it at about 80% of a window per thread | measured in the load test; `events.list` moves to the read replica when it grows |
| Model calls | 500/s at most, 32 at once per workspace (D13), and the account-wide budget (M-4) | bounded by M-1 and M-4 |
| Compaction calls | one per thread each time it reaches the summary threshold; a step that compacts makes no turn call | at most the model call rate, measured in the load test |
| MCP per step | an MCP tool call adds one step (MC-24); 1 `mcp_connections` SELECT and 1 credential SELECT per step; `initialize` and `tools/list` only on a reconnect (MC-15); 1 `GetSecretValue` per server the step connects to or calls (MC-6) | at most 500 secret reads/s when every step calls one server, about 1.3×10^9 a month, about $6.5k a month at $0.05 per 10,000 calls (Q1); a step that calls no MCP tool reads none |
| Swap reads (Cloudflare) | each credential read at most once per 5 minutes per session object (CF-6) | 10^4 sessions × c credentials / 300 s = 33c reads/s |
| Secrets Manager in all | MCP plus swap reads against the fixed 10,000/s regional `GetSecretValue` limit (Q1) | under 600/s with one credential each; the envelope trigger fires at 5,000/s |
| Loop workers | steps in flight = 500/s × model latency; each worker runs up to 256 steps at once (S-8) | about 60 workers at a 30 s latency; the autoscaler follows `agentd_loop_steps_due` |
| Database connections | loop workers and API replicas × a pool of 16 | about 1,000 at 60 workers: past a single instance's comfortable `max_connections`, so RDS Proxy is on before this peak (trigger below) |
| `cloud` sandboxes on Cloudflare | 10^4 sessions × 1/4 to 1/2 vCPU against the 1,500 vCPU default (G1) | 2,500 to 5,000 vCPU: a capacity raise is agreed with Cloudflare before production volume |
| Fargate fleet (AWS-only operators) | one task per active session | the Fargate vCPU and ENI quotas (Q4) are raised with AWS before production volume |

### Scale-up triggers

Each trigger is a CloudWatch alarm that opens a ticket, not a page; the scale-up is a setting change (Deployment).

| Trigger | Metric | Alarm | Action |
|---|---|---|---|
| Postgres connections | `AWS/RDS` `DatabaseConnections` over `max_connections` | `agentd-db-connections-high`: above 70% for 15 minutes; also checked at deploy time as loop workers × pool size | add RDS Proxy (transaction pooling) |
| Secrets Manager reads | `AWS/Usage` `CallCount` for Secrets Manager `GetSecretValue` | `agentd-secrets-reads-high`: above 5,000/s (50% of the limit) for 15 minutes | plan envelope encryption (D30) |
| Log reads | `events.list` p99 latency | `agentd-log-reads-slow`: above 500 ms for 1 hour | configure the read replica |
| Event-log load | events appended per second, and `AWS/RDS` `WriteIOPS` and `CPUUtilization` | `agentd-event-log-hot`: appends above 2,500/s (half the sizing peak) or Postgres CPU above 70% for 1 hour | plan the DynamoDB event log (Out of scope) |
| Sandbox start | time from work item queued to first `user.tool_result` | `agentd-sandbox-start-slow`: p99 above 10 s for 1 hour | a warm buffer on Fargate, or a capacity raise on Cloudflare |

### Operational alarm

`agentd-retention-stale` fires when `agentd_retention_last_success` shows no run for 45 minutes while a retention
period is set; it opens a ticket, and the action is to restart one replica, whose first run walks every archived
session (E4).

### Accepted risks

| Risk | Bound | Reopens when |
|---|---|---|
| A revoked `environment_variable` credential keeps working in a running `cloud` session for up to 5 minutes (CF-6) | 5 minutes, per session object | live documents a shorter re-resolution, or a customer needs immediate revocation |
| The Cloudflare launcher's AWS read key is a static key (G8) | `GetSecretValue` on tagged secrets only, from Cloudflare's IP ranges, rotated every 90 days | Cloudflare documents a Worker identity AWS accepts |
| An API update of an `mcp_oauth` credential made during a refresh's token call (at most 10 s) can have its secret overwritten by the refresh (MC2) | 10 s per refresh, logged as `refresh_overwrote_update`; both tokens come from the same grant | a customer reports a lost update, or the log shows it more than rarely |
| A crash between a session delete's commit and its event delete leaves that session's events (E4) | one session per crash; no API can reach them, since the session answers 404 | the DynamoDB plan, which moves deletes to the store's expiry |
| A loop worker that dies after an MCP server answered a call and before its step commits the result makes the next step call the tool again (MC-25) | one repeated call per crash; the step that calls a tool commits its result at once, with no model call in between | the MCP spec gains idempotency keys, or a customer reports a repeated side effect |
| A sandbox whose networking cannot be read never starts, and its session waits on its tool call until a `user.interrupt` (CF-12) | 10 minutes of retries per work item | the networking read fails outside a Cloudflare or agentd outage |
| On Cloudflare, a placeholder sent on a port other than 80 or 443 reaches its host unchanged, so the call fails there (CF-5) | tools that call credentialed APIs on other ports | a customer needs a credential on another port |
| The Sandbox SDK and backups need the `durable_object` scheduling policy, in public beta (G6) | CF-4's tar-to-bucket fallback | the policy leaves beta, or breaks |

### Load test plan

`perf/` (k6) drives N sessions against the scripted model with 2 to 8 loop workers, and records step-start latency,
claim contention, Postgres statements per second, connections, the log size per step, compaction calls and model
latency. Runs: a steady run; a loop worker killed mid-step; a workspace at its model limit; the account-wide budget
full; long sessions that cross both compaction thresholds. Results go on the loop and compaction PRs (6, 7).

## Audit of this plan

Inventory: the 2026-11-06 inventory of the runtime on 88ab967 (a first pass by reading `work`, `sessions`,
`edge/auth.rs`, `sandbox_work.rs`, `harness.rs` and the migrations; a second, independent pass of 35 patterns, from
`WorkScope` with 22 hits to `SKIP LOCKED` with 0; the reconciliation added the harness reading the 412 detail at the
wrong path, interrupts drained in FIFO order, the conformance suites polling with a workspace key, and three area
audits' findings). A second pass over `git diff 88ab967 origin/main -- crates harness docker` (21 files, all in
`store-postgres`, `docker/sandbox/README.md`) changes no runtime file; it adds that new tables record their writer
through the connection settings from history PR #41. A third pass, for the 2026-11-08 additions, read the files they
touch and their sizes: `vaults/src/service.rs` (304 lines), `store-postgres/src/entities/vaults.rs` (295),
`vaults/src/store.rs` (84), `vaults/src/routes.rs` (136), `deployments/src/service.rs` (308), `deployments/src/types.rs`
(318), `api/src/deps.rs` (181), `sessions/src/service/lifecycle.rs` (162), `shared/src/limits/models.rs` (69),
`secrets-aws/src/name.rs`; and checked what exists: session update already refuses `vault_ids`; session create checks no
vault; credential update replaces the whole `auth`, so `token_endpoint` and the expiry change with it; no
`mcp_oauth_validate` route; `agent.thread_context_compacted` is already an event type; the model catalog holds ids
only; `PausedReason` has only `manual`.

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
| `vaults/src/egress.rs` (4) | hands vault credentials to the egress service | RM3 (D28: placeholders travel in the work secret instead) |
| `crates/egress/` (14), `crates/adapters/egress-proxy/` (2) | the egress service and proxy | move to `agentd-worker` (RM3) |
| `crates/adapters/sandbox-docker/` (9), `crates/sandbox-worker/` | the Docker runtime and relay | move to `agentd-worker` (RM2) |
| `Cargo.toml` members and deps, `api/Cargo.toml`, `docker/sandbox/Dockerfile` | build them | RM2, RM3 |

| Source | Result |
|---|---|
| `~/.claude/CLAUDE.md` | PASS: separate spec, migration and code PRs; `gh stack`; no footer; no em dashes; never push `main` or merge. |
| `rules/planning-phase.md`, `plan-audit` | PASS: inventory, second pass, plan, this audit, questions to the user one at a time. |
| `rules/planning-questions.md` | PASS: every new table is a migration PR the user approves (`mcp_connections` and the checkpoint index follow D32 and D31); access checks read one row; no file over 200 lines grows (W7 splits `edge/auth.rs` into `reach.rs`; PR 2 splits the vault and deployment files before PRs 4 and 8 change them; new code lands in new files); `mcp_oauth_validate` is live's endpoint, not a derived field. |
| `rules/diagnose-before-patch.md` | PASS: the engine is restored, not rewritten; container code leaves agentd instead of growing beside it. |
| `rules/comment-policy.md` | PASS: no comments planned beyond the SQL claim notes. |
| `rules/auto-rerun-ci.md` | Not applicable: applies during the build. |
| Repo `CLAUDE.md` | Not applicable: agentd has none. |
| Memory: agentd parity, no SDK of our own, harness in sandbox, event log behind its port | PASS: the loop in agentd, workers outside, no agentd-only route; every event-log access goes through `SessionEventLog`, so retention and session delete read sessions through their own store (E4). |
| `grill-me` ledger | PASS: no `ASSUMED` row. |
| `industry-practice` | PASS: section above; one departure from live (D24), with its reason; the handoff prompt for other models is beside live, not a departure from it. |
| `review/references/code-design.md` and its four parts | PASS: questions on models (`Session::mounts`), no `else`, files at most 200 lines. |
| `review/references/sql-transactions.md` | PASS: statements per operation; no subqueries; claims with `SKIP LOCKED`; a commit checks its claim first. |
| `review/references/architecture.md`, `system-design.md` | PASS: one path for `cloud` and `self_hosted`; no parallel route. |
| `review/references/checks.md`, script checks | PASS on this text; each PR runs them. |
| `build` (migrations, dependencies) | PASS: one migration PR, indexes on existing tables built `CONCURRENTLY`; the Java SDK from the version catalog. |
| `test` | PASS: each requirement row names its test in its PR. |
| `tech-writing-style`, `no-em-dashes` | PASS: terms defined above; no em dashes; the design stated as fact. |
| `pr-body` | PASS: each PR's body carries these IDs. |
| `tosspaper-testing` | Not applicable as written (tosspaper's stage); each agentd PR records its local and CI verification. |
| SRE skills | PASS: SLOs, sizing, scale-up triggers with metrics and alarms, accepted risks, load test plan, rollout stages, the 2026-11-06 review's findings answered (signals over Valkey, no held lease, one retry layer, reaper as a task, autoscaling on queue depth). |
