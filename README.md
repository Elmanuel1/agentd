# agentd

A modular Rust control plane that is a **drop-in, wire-compatible replacement
for Anthropic's Managed Agents API** (`anthropic-beta: managed-agents-2026-04-01`),
extended with a versioned-memory + dreams layer.

Point any Anthropic SDK at it:

```python
client = anthropic.Anthropic(api_key="agentd-dev-key", base_url="http://localhost:8080")
client.beta.agents.create(name="X", model="claude-sonnet-4-6")   # identical wire
```

## Ground truth, not docs

Every wire shape, limit, and error string was captured from the **live**
Anthropic API (fixtures in `conformance/fixtures/`, captured 2026-07-03) and enforced by
`conformance`. Probing found the docs wrong six times; agentd follows
observed reality:

1. memory update is **POST** (PATCH/PUT → 405 `Allow: DELETE, GET, HEAD, POST`)
2. session ids are `sesn_…`, not `session_…`
3. malformed ids → **400** `Invalid agent ID.`, not 404
4. environment names are **not unique** (dup create → 200)
5. session `metadata` has **no entry cap** (agent metadata caps at 16)
6. `DELETE /v1/environments/{id}` → 200, not 204

## Architecture

```
SDK / ant CLI → api (auth · beta gate · envelopes)
   ├─ crates/resources (one per resource, vertical slices):
   │    agents · environments · vaults(+credentials) · memory(+versions)
   │    files · skills · sessions(+events/SSE) · deployments(+runs) · dreams(+runs)
   ├─ shared           ids · limits (exact strings) · error envelope · cron/DST engine · ports
   ├─ engine           the loop (ModelClient/ToolExecutor/Grader ports, jailed tools)
   ├─ scheduler-local  SchedulerPort backends (Local; Temporal = same contract)
   └─ conformance      the swappability oracle (fixture diff, normalized)
```

**Swappable ports:** `Runtime` (Fargate/Firecracker/local), `SchedulerPort`
(Temporal/EventBridge/local), `ModelClient` (Anthropic HTTP/rig/mock),
storage traits per entity (in-memory/Postgres).

**The novel layer:** memory versions are immutable with a movable `latest`
ref (O(1) rollback); dreams propose candidates, a review gate
(evidence·conflict·novelty) **gates version creation**, and the next session
resolving `latest` adopts the promotion — promote-don't-replace.

## Run

```sh
docker compose -f docker/docker-compose.yml up   # full stack
AGENTD_DEV_KEY=1 AGENTD_S3_CREATE_BUCKET=1 cargo run -p api   # against your own Postgres and S3
cargo test --workspace                           # hermetic; AGENTD_PG_TESTS=1 adds the Postgres and S3 tests
```

Backends and settings: [docs/configuration.md](docs/configuration.md).

## Compile-safety rules (workspace-enforced)

`unsafe_code = forbid`; clippy `unwrap_used/expect_used/panic/indexing_slicing
= deny` in all non-test code. Typed IDs (`Id<Agent>` ≠ `Id<Session>`),
exhaustive event/status enums, `SecretString` with **no `Serialize` impl**
(secret leakage is a compile error), validated newtypes.
