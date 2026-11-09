# Runtime parity: the plan in brief

## Summary

agentd serves Anthropic's Managed Agents API and runs the agent loop on the server, as Anthropic does: it calls the
model, records every event, and queues a work item when a tool has to run. A separate container project, `agentd-worker`, runs
Anthropic's own worker, unchanged, which picks up work items and runs the tools. Our `cloud` environments reuse that
container project: we run it ourselves and point it at agentd, exactly as a customer runs it for a `self_hosted`
environment, with each session's sandbox on Cloudflare Containers. At launch agentd is one container on tosspaper's
existing host and Postgres; bigger setups are settings, each switched on when a named alarm says it is needed.

## How a turn flows

```mermaid
sequenceDiagram
    participant C as Client (SDK)
    participant A as agentd: API and work queue
    participant L as agentd: loop worker
    participant M as Model
    participant W as Worker (agentd-worker)
    C->>A: user.message
    A->>L: wakes the session's loop step
    L->>M: prompt built from the event log
    M-->>L: reply asks for a tool
    L->>A: agent.tool_use, work item queued
    W->>A: polls and takes the work item
    A-->>W: agent.tool_use on the event stream
    W->>W: runs the tool
    W->>A: user.tool_result
    A->>L: wakes the next loop step
    L->>M: prompt with the tool result
    M-->>L: final reply
    L->>A: agent.message, session idle
    A-->>C: every event on the event stream
```

## Terms

- **Live**: Anthropic's hosted Managed Agents API, the behaviour agentd copies.
- **Sandbox**: the container a session's tools run in.
- **Harness**: agentd's current Python loop that runs inside each sandbox and writes the agent's events through
  agentd-only `/harness/*` routes; this plan removes it.
- **Outcome grading**: a second model pass that checks the agent's work against a rubric the caller gave.
- **MCP server**: an outside tool server the agent can call; agentd connects to it with the credential stored in
  the session's vaults.
- **Loop worker**: an agentd server whose job is running loop steps; the **step queue** is the list of sessions
  with a step to run.
- **Work token**: the credential a worker gets with a work item; it reaches only that session.
- **Environment key**: the key a worker uses to ask for work; the **runtime queue** is where `cloud` work waits.
- **Loop step**: one pass of a session's loop: read the session's event log, ask the model, record what it said.
  A tool call ends a step; the tool's result starts the next one.
- **Work item**: a queue entry that tells a worker "this session needs you"; its id is the session id.
- **Lease**: a claim with an expiry. A loop worker holds a step's lease while it runs that step; if it dies, the lease
  runs out and another worker takes the step.
- **`EnvironmentWorker`**: Anthropic's worker, part of the official Python SDK. It takes a work item, runs the
  session's tools, and posts each result back.
- **Sidecar**: on the Fargate launcher, a helper container in the same task as the worker, sharing its disk, that
  copies the session's files in and its outputs out. On Cloudflare the session's Durable Object does this job.
- **Placeholder and swap**: a `cloud` sandbox sees a signed placeholder in place of each `environment_variable`
  credential. When a tool in the sandbox (`curl`, `git`) sends it to one of the credential's allowed hosts, the swap,
  outside the sandbox, puts the real value in. On Cloudflare the swap is the session's Durable Object; elsewhere it
  is an optional egress proxy. `self_hosted` sandboxes get no placeholders, as on live.
- **Durable Object**: Cloudflare's per-name server object; each session's container has one, addressed by the
  session id, which starts the container, stages its files and runs the swap.
- **Compaction**: keeping a long conversation inside the model's window. First old tool results are cleared from the
  prompt; later the conversation is summarised into a checkpoint. Compaction deletes no event.
- **`cloud` and `self_hosted`**: the two kinds of environment. A `self_hosted` environment's workers run on the
  customer's machines; a `cloud` environment's workers run on ours. agentd treats both the same way.

## The PRs

agentd (one stack, each PR on your go-ahead):

1. The spec (this folder).
2. Make room: three large files are split before later PRs change them, with no behaviour change.
3. Migrations: every new table and index in one production-safe PR.
4. The work API for Anthropic's worker: work items in Postgres, live's work secret (with signed placeholders for
   `cloud` environment-variable credentials), environment keys and our runtime key, work tokens that reach only their
   own session, and the session's files for workers.
5. The event log and streams: events in order on every replica through Valkey signals, a fallback timer, new streams
   refused under overload, the event list from a replica, an optional retention period. The log stays on
   Postgres; DynamoDB for large workspaces is a later step, behind the same port, when its alarm fires.
6. The loop: the engine brought back, steps from a Postgres queue with leases, the `api` and `loop` roles, model
   limits per workspace and across the account, and sessions run on the loop behind a setting.
7. Compaction: clear old tool results, then summarise into checkpoints.
8. MCP and vaults: MCP servers called from the loop with cached connections, vault credentials read per step, one
   OAuth refresh at a time, deployments paused when the environment blocks an MCP host, and vaults as Anthropic
   documents them.
9. Scheduled deployments on any replica, and session webhooks.
10. Removals: the Python harness and its routes, agentd's sandbox launch, the egress crates (they move to
    `agentd-worker`).
11. The Java SDK in CI beside Python and TypeScript, SLO alerts, scale-up alarms, the load test, docs and runbook.

`agentd-worker` (its own repository, `Elmanuel1/agentd-worker`, created on your go-ahead):

1. The worker image (the official `anthropic` SDK only, running `EnvironmentWorker` per work item), our runtime
   poller (`ant beta:worker poll --on-work`), the Docker launcher and file staging.
2. The Cloudflare launcher for our `cloud`, built on `@cloudflare/containers`, `@cloudflare/sandbox` and Cloudflare's
   `claude-managed-agents` template (credited): one container per session running the official worker, which alone
   holds the work item; networking read with our runtime key and enforced by `allowedHosts`, failing closed; the
   placeholder swap in the session's Durable Object with values kept 5 minutes in memory; `/workspace` backed up to
   R2; the environment key never inside the container.
3. Later and optional, for AWS-only operators: the Fargate launcher with its queue-driven fleet, the EC2 launcher,
   and the AWS egress proxy.

## Your decisions

- The agent loop runs in agentd, server side (2026-11-05, 2026-11-08).
- agentd must match Anthropic feature for feature (2026-11-08).
- No container code in agentd; the container work is a separate project you will also use elsewhere (2026-11-08).
- `cloud` reuses that container project, run by us (2026-11-08); its sandboxes run on Cloudflare Containers, built on
  Cloudflare's own open-source packages (2026-11-08).
- The container project is called `agentd-worker` (2026-11-08).
- Our `cloud` attaches environment-variable credentials at egress, as Anthropic's cloud does; `self_hosted` gets none;
  for anyone else running `agentd-worker` it is optional and off by default (2026-11-08).
- On Cloudflare the session's Durable Object holds the values, read with one narrow AWS key and kept 5 minutes in
  memory; a revoked value can work for up to 5 minutes (2026-11-08).
- agentd hands the worker signed placeholders in the work item's secret, never values, and no new route
  (2026-11-08).
- The Cloudflare launcher reads a `cloud` environment's networking with our runtime key, the only key allowed to;
  if it cannot, the sandbox does not start (2026-11-08).
- One credential store for now, Secrets Manager; envelope encryption when reads pass half of AWS's limit
  (2026-11-08).
- agentd serves MCP servers from the loop with the session's vault credentials (2026-11-08), read once per loop step
  that uses them, with the MCP session and tool list kept per session in Postgres (2026-11-08).
- Vaults behave as Anthropic documents them (2026-11-08).
- Compaction in two tiers: clear old tool results at 100,000 tokens or half the window, summarise at 150,000 tokens
  or 80% of the window (2026-11-08).
- A session's work token may read its own mounted files and save its outputs through the existing files endpoints,
  a departure from live (2026-11-08).
- No session state held in a replica: steps from a Postgres queue, any replica runs the next step (2026-11-08).
- A lean launch: one agentd container on tosspaper's host and Postgres, no RDS Proxy until the connections alarm
  fires (2026-11-08).
- Environment keys in their own table (2026-11-08).
- No endpoint Anthropic does not have (2026-11-06).
- Custom tools are answered by the API caller (2026-11-04).
- Fan-out signals over Valkey (2026-11-06); the event log stays on Postgres for now, DynamoDB later behind the same
  port (2026-11-08).
- Model limits: 32 model calls at once per workspace, 5 attempts, prompt caching (2026-11-08), and an account-wide
  limit (2026-11-08).
- Webhook endpoints configured per organization (2026-11-08).
- The Java SDK joins the conformance suites; tunnels and user profiles get their own plans (2026-11-08).

## What it costs at launch

For 10,000 extractions a month, excluding the model: about $45 to $125. agentd, Postgres, Valkey and S3 ride on
what tosspaper already runs; Cloudflare is the $5 plan plus $23 to $93 of containers and under $5 of Durable
Objects; staging documents from S3 adds about $15 to $20. The full breakdown is in
[requirements.md](requirements.md), section Deployment.

## Appendix

The full detail lives beside this page:

- [requirements.md](requirements.md): every requirement with its ID (W, F, E, S, R, M, L, CP, FO, K, MC, VA, H, EV, J,
  and `agentd-worker`'s EG, CF, FL), the per-PR files, SQL statements and tests, the step queue's statements,
  rollout, deployment and cost, SRE sizing, scale-up triggers and SLOs, the plan's audit.
- [decisions.md](decisions.md): each decision with its source.
- [dependencies.md](dependencies.md): what the design rests on, with evidence.
- [ADR 0001](adr/0001-agent-loop-in-agentd.md): the decision record and the alternatives.

