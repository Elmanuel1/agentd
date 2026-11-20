# ADR 0001: Webhooks are sent from their rows, by every instance

Status: Proposed
Date: 2026-11-19
Deciders: the user (Elmanuel1)
Scope: agentd's webhook sender (the change feed's delivery handler, the claim loop, endpoint health)

## Context

Each status change agentd reports writes one `webhook_deliveries` row per endpoint, in the commit of the change
(the outbox, runtime-parity D51). On `main`, the change feed's handler POSTs the first attempt inline, with a 10 s
timeout, and keeps retries in memory:

- one slow endpoint holds the feed, and on Kafka the topic has one partition, so one instance does every first send;
- a restart loses every pending retry;
- a failed mark makes the feed redo the first POST, so a receiver can get the same delivery twice from two replicas;
- the table's `next_attempt_at`, `claimed_until` and due index exist and are unused.

## Decision

1. The table is the only send path. On a delivery insert the change feed's handler publishes one wake on Valkey
   `agentd:webhooks` and returns.
2. Every instance runs one claim loop. On a wake, at most once every 50 ms, and every 1 s without one, it claims up
   to its free senders (at most 100) of due rows with `FOR UPDATE SKIP LOCKED`, leaving out endpoints whose breaker
   is open on it, and sets `claimed_until = now() + 60 s` and `attempt = attempt + 1` in the same transaction. The
   update repeats the due conditions, and the instance sends only the rows it returns.
3. A POST starts only while at least 15 s of its claim remain, so a send (10 s timeout) always ends inside its
   claim. Every outcome write is fenced on `attempt`.
4. Attempts, spacing and disable rules match live: three attempts, jittered exponential backoff (15 to 30 s, then 30
   to 60 s), a 3xx or a non-public address disables at once, and 120 h of failure disables (Svix's window, as live
   states none). Endpoint health is a shared row in `webhook_endpoint_states`; the breaker is per replica.
5. There is no per-organization cap. Each instance has 256 senders, a bound on one process.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Svix as the sender | A second service that agentd still POSTs to; it re-serializes the signed body and replaces `webhook-id`. |
| Partition the feed topic by endpoint | Spreads first sends, but a restart still loses retries and a failed mark still sends twice. |
| Keep the feed send path for first attempts and the table for retries | Two send paths, and the feed's partition still decides which instance sends. |
| Retries in memory with a larger pool | Loses retries on restart. |
| Sleep until the next due time instead of polling every 1 s | Saves about one empty indexed read a second per instance; adds a second query and a timing rule (D18). |

## Consequences

- Adding instances adds sending capacity, with no shared counter: Postgres gives each row to one instance.
- Three writes per delivery (insert, claim, outcome) and two more change-feed records, both ignored by the handler.
- A new delivery waits up to about 50 ms for a claim. Deliveries still go out while the change feed is down, found
  by the 1 s poll.
- Delivery stays at least once: an instance that crashes after its POST and before its outcome leaves the claim to
  expire, and another instance sends again with the same `webhook-id`.
- Delivered rows are deleted after 24 h and dropped rows after 30 days, which keeps the table bounded.
- Runtime-parity H-3 is superseded by SH-3; H-4 and D51 narrow to "the change feed wakes the senders; the table is
  the send path".
