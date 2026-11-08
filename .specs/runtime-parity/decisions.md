# Runtime parity: decisions

The initiative makes agentd's runtime match Anthropic Managed Agents feature for feature: the agent loop runs in
agentd and each sandbox runs Anthropic's worker unchanged ([ADR 0001](adr/0001-agent-loop-in-agentd.md)). Each
decision names who made it and when; a later decision replaces an earlier one on the same point.

## Architecture

| # | Decision | Source |
|---|---|---|
| D1 | The agent loop runs in agentd, server side. | User, 2026-11-05 ("Include the loop here"); 2026-11-08 ("must match anthropic feature for feature") |
| D2 | The sandbox runs the official SDK's `EnvironmentWorker` unchanged, through its documented per-item entry `worker(workdir="/workspace").handle_item()`, started by Anthropic's `ant beta:worker poll --on-work` poller. No agentd code runs in the sandbox. Replaces `ant beta:worker run` in the sandbox (2026-11-04). | User, 2026-11-08 |
| D3 | agentd serves only the endpoints and event types Anthropic serves; no agentd-only route, no new endpoint. | User, 2026-11-06 ("dont add an endpoint anthropic never added"), 2026-11-08 |
| D4 | Custom tools are answered by the sessions API caller (`user.custom_tool_result`). The worker runs the built-in toolset only, as it does by default. | User, 2026-11-04 |
| D5 | Everything `cloud` runs through agentd's own poller and a per-session dispatcher: file staging, output upload, `/workspace` snapshots. | User, 2026-11-05 ("All of it, one path") |
| D6 | Production sandboxes are ECS Fargate tasks behind the `Runtime` port; Docker in development and CI only. | User, 2026-11-06 |

## The loop at scale

| # | Decision | Source |
|---|---|---|
| D7 | No session state in a replica's memory. The event log in Postgres is the state; a step reads it, builds the prompt, calls the model and appends its events in one commit. A crash repeats at most one step, which is safe to repeat. Replaces the run lease held while waiting and the per-replica history cache (2026-11-06, Q7). | User, 2026-11-08 ("okay. lets do that") |
| D8 | Steps come from a queue. A session joins it when a client event, `user.tool_result` or `user.custom_tool_result` arrives; a loop worker claims it with `FOR UPDATE SKIP LOCKED` under a lease with an expiry, runs one step and releases it. Nothing is held while a tool runs. No subqueries. | User, 2026-11-08 |
| D9 | One binary, two roles: the API (requests and streams) and loop workers (steps). The role is a setting; each scales on its own, loop workers on queue depth. | User, 2026-11-08 |
| D10 | Model provider limits: a per-workspace concurrency limit, retries with backoff on 429 and 5xx in the model client only, prompt caching where the provider supports it. Settings come from per-environment config. | User, 2026-11-08 |
| D11 | Scheduled jobs work with more than one replica: schedules live in Postgres and each due fire is claimed by one replica. | User, 2026-11-08 |

## Data and keys

| # | Decision | Source |
|---|---|---|
| D12 | Work items, leases and hand-outs live in Postgres (`work_items`); a session's work item id is its session id. | User, 2026-11-05 (Q3) |
| D13 | Environment keys (`sk-ant-oat01-`) are stored as peppered digests in their own `environment_keys` table, made with `agentd environment-keys`. Replaces the column on `api_keys` (Q4). | User, 2026-11-08 ("Yes, stored keys") |
| D14 | An environment key is a Bearer token only, bound to one organization, workspace and environment. | User, 2026-11-08 |
| D15 | Fan-out signals ("session S changed", "queue Q has work") go over Valkey pub/sub; the log stays the source of truth and each stream falls back to a 1 s timer. | User, 2026-11-06 (Q6) |
| D16 | The event log stays behind its port and moves to DynamoDB for flagged workspaces in this stack; no partitioning. | User, 2026-11-06 (Q8, Q12) |
| D17 | The work secret carries live's keys and values. | User, 2026-11-06 (Q5) |
| D18 | Starter SLOs as signed off on 2026-11-06, targets in one config file. | User, 2026-11-06 (Q14) |
| D19 | No paid model run without the user's OK; CI uses the scripted model. | User, standing |

## Settled with the plan

| # | Decision | Source |
|---|---|---|
| D20 | Session webhook endpoints are registered through agentd configuration per organization, since live registers them only in its Console; deliveries follow live exactly. | User, 2026-11-08 (plan approved) |
| D21 | Model limits default to 32 concurrent model calls per workspace, 5 attempts from 2 s with jitter inside a 10 min deadline, and `cache_control` on the system prompt and tool list for Anthropic models; each is a setting. | User, 2026-11-08 (plan approved) |
| D22 | The plan of PRs in [requirements](requirements.md) is agreed; each PR starts on the user's go-ahead. | User, 2026-11-08 |
