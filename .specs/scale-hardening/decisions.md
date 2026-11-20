# Scale hardening: decisions

agentd holds up under load and across replicas: webhooks are sent from their rows by every instance
([ADR 0001](adr/0001-webhooks-sent-from-rows.md)), every wait and every statement is bounded, the API limits each
organization as live does, and the tables that grow are swept. "Live" means Anthropic's hosted Claude Platform at
api.anthropic.com. Each row names who decided and when.

## Asked and answered

| # | Decision | Source |
|---|---|---|
| D1 | Webhooks are sent only from their `webhook_deliveries` rows, by every instance. The change feed only wakes the instances; Postgres `FOR NO KEY UPDATE SKIP LOCKED` is the coordinator. There is no per-organization cap. | User, 2026-11-19 ("Yes, switch"; "for now, I don't think we really need it") |
| D2 | A retry waits by exponential backoff with jitter: a 30 s base doubled per attempt, half fixed and half random, so 15 to 30 s after the first attempt and 30 to 60 s after the second, inside live's 5 to 120 s. | Live, L1; user, 2026-11-19 (a backoff that grows per attempt) |
| D5 | Svix is not used. agentd would still POST to it, and it re-serializes the signed body and replaces `webhook-id`. Its fault-tolerance features (durable rows, backoff, disable, breaker) are built in. | User, 2026-11-19 |
| D6 | A disabled endpoint is re-enabled by `agentd webhooks enable <org> <endpoint>`, or by a change to its URL or secret in `AGENTD_WEBHOOK_ENDPOINTS`, seen at boot by its SHA-256 digest. | User, 2026-11-19 ("yes, go with that") |
| D8 | An API key resolves from a 60 s in-process cache of 10,000 entries. A revoke reaches every replica through the change feed's `api_keys` handler, which publishes on Valkey `agentd:keys`, so a CLI revoke is covered too. | User, 2026-11-19 ("Cache + revoke broadcast") |
| D10 | Two code PRs: about 110 changed files with tests do not fit one PR under 100. | `feedback-agentd-fewer-prs`; user, 2026-11-19 (plan approved) |
| D14 | There is no fixed in-flight cap. When no Postgres connection comes free in time, the edge answers 529 `overloaded_error` on every route; background work retries and never answers 529. | User, 2026-11-19 ("Yes, confirm") |
| D16 | Model calls go through a breaker per provider per replica that makes steps wait, not fail. | User, 2026-11-19 ("Add both") |
| D17 | MCP `initialize`, `tools/list` and token refresh have no in-call retry. A failure is a `session.error` marked `retrying` and the next turn tries again, as live (runtime-parity MC-14). | User, 2026-11-19 ("Yes, remove them") |
| D18 | The webhook claim loop polls every 1 s behind the wake. It does not sleep until the next due time. | User, 2026-11-19 ("Keep the 1 s poll") |
| D19 | Two instances never hold one delivery: the claim's select and update run in one transaction, the update repeats the due conditions, all times are Postgres `now()`, and a POST starts only while 15 s of its claim remain. | User, 2026-11-19 ("Make sure that two instances don't claim the same thing") |
| D20 | The migrations V24 to V27 and the `webhook_endpoint_states` table are approved. | User, 2026-11-19 (plan approved) |

## Matched to live

| # | Decision | Source |
|---|---|---|
| D3 | Three attempts, then the delivery is dropped. A 3xx or a non-public address disables an endpoint at once; sustained failure disables it after 120 h, Svix's default, as live states no window. Events emitted while disabled are not replayed. | L1; DEP-8 |
| D7 | Rate limits as live: a fixed 60 s window per organization and bucket, starting at the window's first request; create 300, read 1,200, memory stores 30,000, files 500; the probed route map; live's headers; 429 `rate_limit_error` with `retry-after`. | L2; DEP-1 to DEP-5 |
| D9 | Retention: memory versions 30 days, except the current version of a live memory; expired files listed for 30 days, their content 404; sessions never by age. | L4 to L6 |
| D21 | A request whose response has not started by its deadline is 504 `timeout_error`. | L3 |
| D22 | The webhook sender sends with `user-agent: Claude-Platform`. | L1 |

## Taken from the standing rules

| # | Decision | Source |
|---|---|---|
| D4 | Each replica keeps its own breakers in memory. | Design skill, user 2026-11-08 |
| D11 | Settings are environment variables, five of them, all `_MS`; every other value is a constant. agentd exports metrics and defines no alarm. A log line's level says who must act. | Standing answers, 2026-11-11 and 2026-11-12 |
| D12 | Migrations carry no data guards and no TRUNCATE. | `project-agentd-not-in-production`, `feedback-no-truncate` |
| D13 | The pool setting keeps the code's name, `AGENTD_DB_POOL_SIZE`; runtime-parity S-8 follows. | Code first, spec last (design skill) |
| D15 | A scheduled fire's session create takes no rate-limit bucket here; follow-up F-1 adds it. | Scheduler scope (`.specs/durable-scheduler/`) |
| D23 | Key revokes and webhook wakes start from the change feed; each action still does its own work inline. | `feedback-wal-for-after-effects` |
| D24 | Delivered webhooks are kept 24 h and dropped ones 30 days, then swept. Both are kept for a retention period, failed rows visible meanwhile; 24 h covers a change-feed replay, and 30 days of all rows would be about 1.5 TB at the design point. | sql-transactions rule 15; sizing in [requirements.md](requirements.md) |

## Decided by Claude

No source settles these; each is listed for review.

- The values in [requirements.md](requirements.md), "Values".
- One sweeper under a transaction-level advisory lock.
- `endpoint_key` alone as the key of `webhook_endpoint_states`: endpoint ids are unique across organizations.
- An open breaker spends no attempt: its endpoint's deliveries are not claimed.
- The events list read in two phases: processed events in index order, then unprocessed ones.
- L4's "recent versions" read as the current version of each live memory.
- `fire_claims` keeps the scheduler's own sweep until follow-up F-6.
- Environment and admin keys are not cached (follow-up F-4).

## Follow-ups

| # | Follow-up |
|---|---|
| F-1 | Fired session creates take the create bucket, which makes `session_rate_limited` reachable. |
| F-2 | The checkpoint-bounded engine read (runtime-parity). |
| F-3 | Batched blob deletes for skills and memory heads. |
| F-4 | Environment and admin key caches, if measured. |
| F-5 | Several signing secrets per endpoint, if live adds them. |
| F-6 | `fire_claims` retention moves into the sweeper. |
| F-7 | A per-organization fair share in the webhook claim, from the load test's numbers. |
| F-8 | The busy-workspace claim and its index (`loop_steps/claim.rs:25-56`). |
| F-9 | One `set_config` round trip per checkout (`checkout.rs:67-81`). |
| F-10 | The credential metadata GET reading the secret, and vault delete purging credentials one by one. |
| F-11 | Rubric files read on every step. |
