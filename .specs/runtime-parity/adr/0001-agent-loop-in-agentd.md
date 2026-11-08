# ADR 0001: The agent loop runs in agentd; the sandbox runs Anthropic's worker unchanged

Status: Proposed (2026-11-08)

## Context

agentd is a self-hosted clone of Anthropic Managed Agents: the official SDKs and the `ant` CLI work against it with
only the base URL changed. Two terms used below:

- The **agent loop** is the code that calls the model, decides which tools to run, and writes the session's
  `agent.*`, `span.*` and thread events.
- The **sandbox** is the container a session's tools run in.

In Anthropic's architecture the loop runs on the server. A sandbox runs the official SDK's environment worker
(`anthropic.lib.environments.EnvironmentWorker`, SDK 2.3.0): it takes a work item with the item's sessions token,
downloads skills, mounts and syncs memory stores, runs each built-in `agent.tool_use`, and posts `user.tool_result`.
The worker never writes an agent-side event.

On `main`, agentd's loop is a Python harness inside each sandbox. To write the agent's events it needs routes
Anthropic does not have (`/v1/sessions/{id}/harness/*`) or SDK extensions, and its conversation lives in the
sandbox's memory.

## Decision

1. The agent loop runs in agentd, server side, as Anthropic's does.
2. The sandbox runs the official SDK's worker unchanged. A `cloud` session's work is claimed by Anthropic's own
   `ant beta:worker poll --on-work` poller, run by agentd with a runtime environment key; the `--on-work` command
   starts one sandbox per session whose entry is the SDK's documented
   `client.beta.environments.work.worker(workdir="/workspace").handle_item()`. No code of agentd's runs in the
   sandbox. A `self_hosted` session's work is polled by the customer's own worker.
3. agentd serves exactly the endpoints and event types that worker uses, with Anthropic's behaviour. There is no
   agentd-only route and no widening of `events.send` for agent events.
4. The loop keeps no session state in a replica's memory. The session's event log in Postgres is the state; one
   loop step reads it, calls the model and appends its events in one commit; steps come from a queue claimed with
   `FOR UPDATE SKIP LOCKED` under a lease, so any replica runs the next step and nothing is held while a tool runs.

## Consequences

- The Python harness, its CI job and the `/harness/*` routes are removed.
- agentd holds the model key and calls the model through its own client; the sandbox never sees it.
- Every built-in tool call round-trips through the event log: the loop parks it as `agent.tool_use`, the worker
  answers with `user.tool_result`, and the result queues the session's next step.
- agentd runs two roles from one binary, the API and the loop workers, each scaled on its own.
- PR Elmanuel1/agentd#44 (the harness on the official SDK, with agent events from the session token) is
  superseded.
