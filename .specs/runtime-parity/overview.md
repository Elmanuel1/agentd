# Runtime parity: the plan in brief

## Summary

agentd serves Anthropic's Managed Agents API and runs the agent loop on the server, as Anthropic does: it calls the
model, records every event, and queues a work item when a tool has to run. A separate container project runs
Anthropic's own worker, unchanged, which picks up work items and runs the tools. Our `cloud` environments reuse that
container project: we run it ourselves and point it at agentd, exactly as a customer runs it for a `self_hosted`
environment.

## How a turn flows

```mermaid
sequenceDiagram
    participant C as Client (SDK)
    participant A as agentd: API and work queue
    participant L as agentd: loop worker
    participant M as Model
    participant W as Worker (container project)
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
- **Loop step**: one pass of a session's loop: read the session's event log, ask the model, record what it said.
  A tool call ends a step; the tool's result starts the next one.
- **Work item**: a queue entry that tells a worker "this session needs you"; its id is the session id.
- **Lease**: a claim with an expiry. A loop worker holds a step's lease while it runs that step; if it dies, the lease
  runs out and another worker takes the step.
- **`EnvironmentWorker`**: Anthropic's worker, part of the official Python SDK. It takes a work item, runs the
  session's tools, and posts each result back.
- **Sidecar**: a helper container in the same task as the worker, sharing its disk. Here it copies the session's
  files in and its outputs out.
- **Egress proxy**: the gateway every outbound request from a sandbox goes through. It swaps placeholder credentials
  for real ones, so the sandbox never holds a secret.
- **`cloud` and `self_hosted`**: the two kinds of environment. A `self_hosted` environment's workers run on the
  customer's machines; a `cloud` environment's workers run on ours. agentd treats both the same way.

## The PRs

agentd (one stack, each PR on your go-ahead; ✔ marks a PR that removes code):

1. The spec (this folder).
2. Work tokens keep working across restarts: their signing key lives in Secrets Manager.
3. Add the `work_items` table.
4. A Postgres store for work items, not used yet.
5. Work items move to Postgres; a work item's id is its session id.
6. The work item's secret carries the same fields as live's.
7. Add the `environment_keys` table.
8. Environment keys: create them with a command; poll only with one; live's 401 and 403 answers.
9. A session waiting on a tool result refuses other messages, as live does.
10. Add an ordered sequence number and a processed time to the event log.
11. Mark events processed in one statement; read the log in order without repeats.
12. Events reach every stream on every replica, through Valkey signals.
13. Move the storage settings into their own config file.
14. DynamoDB event log: writes.
15. DynamoDB event log: reads.
16. DynamoDB event log for the workspaces you list.
17. Add the `loop_steps` table.
18. Bring the model client back: one retry layer, prompt caching.
19. Bring back the event history and the prompt.
20. Bring back the turn: a tool call waits for a worker.
21. Bring back threads, delegation and interrupts.
22. Bring back outcome grading and the budget.
23. The step queue: claim a step, commit it, take over a dead one.
24. Loop workers run steps; the `api` and `loop` roles; a model-call limit per workspace.
25. Work tokens can only answer their own session's open tool calls and read its own files and memory.
26. Workers read the session's mounted files and save its outputs with the work token.
27. Sessions run on agentd's loop and queue their work; agentd starts no sandbox (per workspace, behind a setting).
28. Add the `schedules` table.
29. Scheduled deployments fire on any replica, once.
30. Add the webhook tables.
31. Session status webhooks, signed and retried as live does.
32. ✔ Remove the Python harness, the `/harness/*` routes and the harness CI job.
33. ✔ Remove agentd's sandbox launch and the `sandbox-docker` and `sandbox-worker` crates (they move to the container project).
34. ✔ Remove the egress proxy crates from agentd (they move to the container project).
35. Docs and runbook.

The container project (its own repository):

1. The worker image: the official `anthropic` SDK only, running `EnvironmentWorker` per work item.
2. The command `ant beta:worker poll --on-work` runs: start one task per work item.
3. The Docker launcher, for development.
4. The Fargate launcher, for production.
5. The file sidecar: copy the session's files in before the worker starts.
6. The file sidecar: save outputs and a `/workspace` snapshot when the session finishes a turn and at exit, with no
   AWS credentials in the task (presigned S3 URLs only).
7. The egress proxy with credential placeholders (moved from agentd).
8. Our `cloud` deployment: workers polling agentd's runtime queue with our key.

## Your decisions

- The agent loop runs in agentd, server side (2026-11-05, 2026-11-08).
- agentd must match Anthropic feature for feature (2026-11-08).
- No container code in agentd; the container work is a separate project you will also use elsewhere (2026-11-08).
- `cloud` reuses that container project, run by us (2026-11-08).
- No session state held in a replica: steps from a Postgres queue, any replica runs the next step (2026-11-08).
- Environment keys in their own table (2026-11-08).
- No endpoint Anthropic does not have (2026-11-06).
- Custom tools are answered by the API caller (2026-11-04).
- Fan-out signals over Valkey; the event log moves to DynamoDB for listed workspaces (2026-11-06).
- Model limits: 32 model calls at once per workspace, 5 retries, prompt caching (2026-11-08).
- Webhook endpoints configured per organization (2026-11-08).

## Open questions

- **The container project's repository name.** Yours to choose.
- **Where the egress proxy gets real credential values.** Live has no public API that returns vault secrets.
  Recommendation: our `cloud` deployment reads them from Secrets Manager with its own IAM role; a customer's
  `self_hosted` deployment is given its own.

## Appendix

The full detail lives beside this page:

- [requirements.md](requirements.md): every requirement with its ID (W, F, E, S, R, M, L, FO, K, H), the per-PR
  files, SQL statements and tests, the step queue's statements, rollout, SRE sizing and SLOs, the plan's audit.
- [decisions.md](decisions.md): each decision with its source.
- [dependencies.md](dependencies.md): what the design rests on, with evidence.
- [ADR 0001](adr/0001-agent-loop-in-agentd.md): the decision record and the alternatives.

Review findings on this spec: round 1 found 16 (all fixed); round 2 is under way.
