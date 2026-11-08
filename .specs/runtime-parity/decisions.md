# Runtime parity: decisions

agentd's runtime matches Anthropic Managed Agents feature for feature: the agent loop runs in agentd, and workers
running Anthropic's own worker live outside agentd ([ADR 0001](adr/0001-agent-loop-in-agentd.md)). "Live" means
Anthropic's hosted Managed Agents API. Each row names who decided and when.

## Architecture

| # | Decision | Source |
|---|---|---|
| D1 | The agent loop runs in agentd, server side. | User, 2026-11-05 and 2026-11-08 ("must match anthropic feature for feature") |
| D2 | Workers run the official SDK's `EnvironmentWorker` unchanged, started per work item through its documented entry `worker(workdir="/workspace").handle_item()`. | User, 2026-11-08 |
| D3 | agentd serves only the endpoints and event types live serves; no agentd-only route and no endpoint live does not have. | User, 2026-11-06 and 2026-11-08 |
| D4 | Custom tools are answered by the sessions API caller (`user.custom_tool_result`); workers run the built-in toolset only, as they do by default. | User, 2026-11-04 |
| D5 | agentd is the API, the loop and the work queue. It holds no container code: no launcher, no runtime adapter, no sidecars, no dispatcher, no file staging, no snapshots, no egress proxy. | User, 2026-11-08 ("This self container implementation is not for agentd ... don't mix it together") |
| D6 | The worker image, its Docker and Fargate launchers, the file sidecar and the egress proxy with placeholders are a separate container project, `agentd-worker`, in its own repository, speaking only the public API, so it runs against live as well as agentd. | User, 2026-11-08 (name chosen the same day) |
| D7 | `cloud` environments are served by our own deployment of the container project: its workers poll agentd's runtime queue with our environment key, as a customer's workers poll their environment. `cloud` and `self_hosted` differ only in who runs the workers and holds the key. | User, 2026-11-08 ("We can reuse our self host as the cloud right": yes) |

## The loop at scale

| # | Decision | Source |
|---|---|---|
| D8 | No session state in a replica's memory. The event log in Postgres is the state; a step reads it, builds the prompt, calls the model, and appends its events in one commit. A crash repeats at most one step, which is safe to repeat. | User, 2026-11-08 ("okay. lets do that") |
| D9 | Steps come from a queue. A session joins it when a client event, `user.tool_result` or `user.custom_tool_result` arrives; a loop worker claims it with `FOR UPDATE SKIP LOCKED` under a lease with an expiry, runs one step and releases it. Nothing is held while a tool runs. No subqueries. | User, 2026-11-08 |
| D10 | One binary, two roles: the API (requests and streams) and loop workers (steps). The role is a setting; each scales on its own, loop workers on queue depth. | User, 2026-11-08 |
| D11 | Model provider limits: a per-workspace concurrency limit, retries with backoff on 408, 409, 429 and 5xx in the model client only, prompt caching where the provider supports it; each value a setting per deployment environment. | User, 2026-11-08 |
| D12 | Scheduled jobs work with more than one replica: schedules live in Postgres and one replica claims each due fire. | User, 2026-11-08 |
| D13 | Model limit defaults: 32 concurrent model calls per workspace, 5 attempts from 2 s with jitter inside a 10 min deadline, `cache_control` on the system prompt and tool list for Anthropic models. | User, 2026-11-08 (plan approved) |

## Data and keys

| # | Decision | Source |
|---|---|---|
| D14 | Work items, leases and hand-outs live in Postgres (`work_items`); a session's work item id is its session id. | User, 2026-11-05 |
| D15 | Environment keys (`sk-ant-oat01-`) are stored as peppered digests in their own `environment_keys` table, made with `agentd environment-keys`; agentd mints its runtime key at boot into Secrets Manager. | User, 2026-11-08 ("Yes, stored keys") |
| D16 | An environment key is a Bearer token only, bound to one organization, workspace and environment (or to the runtime queue). | User, 2026-11-08 |
| D17 | Fan-out signals ("session S changed", "queue Q has work") go over Valkey pub/sub; the log stays the source of truth and each stream falls back to a 1 s timer. | User, 2026-11-06 |
| D18 | The event log stays behind its port and moves to DynamoDB for listed workspaces in this stack; no partitioning. | User, 2026-11-06 |
| D19 | The work secret carries live's keys and values. | User, 2026-11-06 |
| D20 | Session webhook endpoints are registered through agentd configuration per organization, since live registers them only in its Console; deliveries follow live exactly. | User, 2026-11-08 (plan approved) |
| D21 | Starter SLOs as signed off on 2026-11-06, targets in one config file. | User, 2026-11-06 |
| D22 | No paid model run without the user's OK; CI uses the scripted model. | User, standing |
| D23 | Each PR in [requirements](requirements.md) starts on the user's go-ahead; the `agentd-worker` repository is created on the user's go-ahead. | User, 2026-11-08 |
| D24 | A session's work token may read its session's mounted files through `GET /v1/files/{id}/content` and save outputs through `files.upload`, scoped to that session so `files.list(scope_id=session)` returns them; any other session's file, and a file not mounted in the session, are refused. No new route. Live refuses this; it departs from live because agentd holds no container code (D5) and `agentd-worker` speaks only the public API (D6), so its sidecar can stage files only through the files endpoints. | User, 2026-11-08 ("Yes") |
| D25 | agentd has no part in egress secrets: no proxy role, no placeholders handed to a sandbox; vault credentials stay server side, where the loop uses them for MCP servers; the model key never leaves agentd. `agentd-worker`'s egress proxy enforces the environment's `limited` networking by default; secret injection with placeholders is an optional feature its operator turns on with their own secret source; our `cloud` deployment leaves it off. A `cloud` session's `environment_variable` credentials are therefore not substituted, a departure from live's `cloud` sandboxes. | User, 2026-11-08 ("It's up to them") |
| D26 | The loop serves the agent's MCP servers for every environment type, with credentials from the session's vaults as Anthropic picks them. | User, 2026-11-08 |
| D27 | The loop reads each MCP credential once per loop step, not per request; a rotated or revoked credential applies from the next step. Why: 2 to 3 times fewer Secrets Manager reads, the secret held a few seconds in memory inside agentd only, and large multi-tenant systems cache credentials for minutes. | User, 2026-11-08 ("Go ahead") |
