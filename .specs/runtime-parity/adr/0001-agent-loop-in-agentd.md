# ADR 0001: The agent loop runs in agentd; workers run Anthropic's worker outside agentd

Status: Accepted
Date: 2026-11-08
Deciders: the user (Elmanuel1)
Scope: agentd (the API, the loop and the work queue); the container project `agentd-worker` is out of scope

## Context

agentd is a self-hosted clone of Anthropic Managed Agents: the official SDKs and the `ant` CLI work against it with
only the base URL changed. "Live" below means Anthropic's hosted Managed Agents API. Terms:

- The **agent loop** calls the model, decides which tools to run, and writes the session's `agent.*`, `span.*` and
  thread events.
- A **worker** is the process that runs a session's tools: the official SDK's `EnvironmentWorker` (SDK 2.3.0). It
  takes a work item with its **work token** (the item's `sessions_token`, the only credential a worker holds), downloads skills, mounts and syncs memory stores, runs each
  built-in `agent.tool_use`, and posts `user.tool_result`. It never writes an agent-side event.
- A **work item** is the queue entry that tells a worker a session needs it.

On live the loop runs on the server and workers run tools. A `self_hosted` environment's workers belong to the
customer; a `cloud` environment's workers belong to Anthropic.

## Decision

1. The agent loop runs in agentd, server side.
2. agentd is the API, the loop and the work queue. It holds no container code: no sandbox launcher, no runtime
   adapter, no file staging, no snapshots, no egress proxy.
3. For `cloud` and `self_hosted` environments alike, agentd queues work items and serves exactly the work,
   session, events, skills and memory store endpoints the official worker calls, with live's behaviour. A session's
   work token also reads its session's mounted files and saves its outputs through the files endpoints, for
   `agentd-worker`'s sidecar; live refuses this (decision D24). A `cloud` session's `environment_variable`
   credentials are not substituted at egress (decision D25). These are the two departures from live.
4. Workers run outside agentd, in a separate container project, `agentd-worker`, that speaks only the public API. A customer runs it
   for `self_hosted` environments; we run it for `cloud` environments, polling agentd's runtime queue with our own
   environment key. `cloud` and `self_hosted` differ only in who runs the workers and holds the key.
5. The loop keeps no session state in a replica's memory. The event log in Postgres is the state; a step reads it,
   calls the model, and appends its events in one commit; steps come from a Postgres queue claimed with
   `FOR UPDATE SKIP LOCKED` under an expiring lease.

## Alternatives considered

| Alternative | Why not |
|---|---|
| The loop in a harness inside each sandbox, writing agent events through agentd-only routes | Not live's architecture; needs routes live does not have; the conversation lives in the sandbox's memory and dies with it. |
| The loop in the sandbox, posting agent events through `events.send` from the work token | Workers would write agent-side events, so the loop and the conversation move into the sandbox and Anthropic's `EnvironmentWorker` could no longer run unchanged. |
| agentd launches sandboxes itself (Docker, Fargate) | Ties agentd to one container platform and puts container code in the API; the worker contract already lets any runner serve `cloud` work. |

## Consequences

- The Python harness, its CI job and the `/v1/sessions/{id}/harness/*` routes leave agentd.
- agentd's sandbox launch, the `sandbox-docker` and `sandbox-worker` crates, and the egress proxy move to the
  container project.
- agentd holds the model key and calls the model through its own client.
- agentd serves the agent's MCP servers from the loop, with credentials from the session's vaults; no secret
  reaches a worker.
- Every built-in tool call round-trips through the event log: the loop parks it as `agent.tool_use`, a worker answers
  with `user.tool_result`, and the result queues the session's next step.
- agentd runs two roles from one binary, the API and loop workers, each scaled on its own.
