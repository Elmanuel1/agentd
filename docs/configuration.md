# Configuration

The binary reads its configuration from the environment once, at boot
(`api::Config::from_lookup`). Nothing else in agentd reads the environment,
except the OpenTelemetry SDK, which reads the standard `OTEL_*` variables.
Every port gets one backend, chosen by a selector variable; an unknown value
stops the boot with an error naming the variable. No setting turns a security
property off: there is no development mode, and a test gets what it needs from
fakes in test code, never from configuration ([Security](#security)).

## Backends

| Port | Selector | Values (default first) |
|---|---|---|
| Record store | `AGENTD_STORE` | `postgres` |
| Blob store | `AGENTD_BLOBS` | `s3` |
| Secret store | `AGENTD_SECRETS` | `aws`: one AWS Secrets Manager secret per credential, encrypted with a customer-managed KMS key (see [Vault credentials](#vault-credentials)) |

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `AGENTD_ADDR` | `0.0.0.0:8080` | Listen address |
| `DATABASE_URL` | required | Postgres connection |
| `AGENTD_PG_READ_URL` | unset (the primary) | A Postgres read replica the events list (`GET /v1/sessions/{id}/events` and a thread's events) reads; every write, stream and other read uses `DATABASE_URL`. A list may lag the primary by the replica's delay |
| `AGENTD_VALKEY_URL` | required | The Valkey (or Redis) every replica signals session changes through, on the `agentd:sessions` pub/sub channel, so a stream on one replica sees events written on another, and reads the change streams from; `rediss://` for TLS |
| `AGENTD_KAFKA_BROKERS` | unset | Kafka's brokers (`host:port,…`), from whose topics agentd reads the change feed; unset, it reads it from Valkey Streams on `AGENTD_VALKEY_URL` |
| `AGENTD_VALKEY_CLUSTER` | `false` | `true` when `AGENTD_VALKEY_URL` is a cluster's configuration endpoint (cluster mode enabled): agentd then follows the cluster's slots and subscribes over RESP3 |
| `AGENTD_CHANGES_PREFIX` | `agentd` | Debezium's `topic.prefix`: agentd reads the change feed's tables from `<prefix>.public.<table>`, the delete messages from `<prefix>.message` and an external scheduler's fires from `<prefix>.fires` |
| `AGENTD_CHANGES_SLOT` | `agentd_changes` | Debezium's replication slot, which every replica reads every 10 s into the `agentd.feed.slot.*` gauges (`docs/history.md`) |
| `AGENTD_EVENT_RETENTION_DAYS` | unset (events kept) | Archived sessions lose their events this many days after archiving. Every 5 minutes one replica deletes them, walking an hour back from the cutoff (a process's first run walks from the oldest); `agentd.retention.last_success` records each run |
| `AGENTD_DB_POOL_SIZE` | `10` | Postgres connections per replica |
| `AGENTD_DB_POOL_WAIT_MS` | `5000` | Longest wait for a free connection, and for opening or recycling one; past it the request fails instead of queueing |
| `AGENTD_BLOB_BUCKET` | `agentd-artifacts` | The bucket every blob lives in |
| `AGENTD_S3_REGION` | `AWS_REGION`, `AWS_DEFAULT_REGION` or the profile's, else `us-east-1` | Bucket region |
| `AGENTD_AWS_ENDPOINT` | unset (AWS S3) | Custom S3 endpoint (MinIO, LocalStack): `https://` to any host, or `http://` only to a loopback address, `localhost` or a single-label container name |
| `AGENTD_S3_PATH_STYLE` | `1` with a custom endpoint, else `0` | Path-style bucket addressing |
| `AGENTD_S3_ROLE_ARN` | unset | Role to assume with the default-chain credentials |
| `AGENTD_S3_CREATE_BUCKET` | `0` | Create the bucket at boot when it is missing |
| `AGENTD_S3_LOCATION_CONSTRAINT` | the region on AWS S3 (none in `us-east-1`); none with a custom endpoint | The location constraint a created bucket gets, for an S3-compatible store that names locations its own way |
| `AGENTD_API_KEY_PEPPERS` | required | `<id>:<hex>,…`: the peppers every API key digest is keyed with (see [Keys from the environment](#keys-from-the-environment)) |
| `AGENTD_WORK_TOKEN_KEYS` | required | `<id>:<hex>,…`: the first signs work tokens, every one verifies them |
| `AGENTD_PLACEHOLDER_KEYS` | required | `<id>:<hex>,…`: the same, for the placeholders of `environment_variable` credentials |
| `AGENTD_RUNTIME_KEYS` | unset (no runtime key) | agentd's runtime keys, `sk-ant-oat01-…`, comma separated, the current one first |
| `AGENTD_ANTHROPIC_API_KEY` | unset (no Anthropic models) | The loop's key for the Messages API |
| `AGENTD_OPENROUTER_API_KEY` | unset (no OpenAI-compatible models) | The loop's key for the API at `AGENTD_MODEL_BASE_URL` |
| `AGENTD_WEBHOOK_ENDPOINTS` | unset (no webhook is sent) | Each organization's webhook endpoints, a JSON array of `{"id", "organization_id", "url", "secret": "whsec_…", "enabled"}` (see [Webhooks](#webhooks)) |
| `RUST_LOG` | `info` | Log level |


## The loop

Every session runs on agentd's own loop. A client event, or a worker's
`user.tool_result`, puts its session on the step queue (`loop_steps`) in the
same commit. A loop worker claims the step under a lease,
builds one model call's prompt from the session's event log, and commits what
the reply leads to:
- the call's start, the running status and the client events it takes, in one
  commit before the call;
- its events, in one commit after it.

While a step runs, its worker renews the lease every third of its length, so
slow MCP or model work never outlasts it; a step whose claim is gone (another
worker took it, or the session was deleted) stops where it is and sends
nothing more. Nothing is held in a replica between steps. A step cut off by a
crash renews nothing, so it runs again from the log once its lease runs out. A
`user.interrupt` sent during a
call cancels it. Built-in tool calls are parked for the session's worker,
which takes the session's work item:
- a `self_hosted` session idles `requires_action` until its worker posts
  `user.tool_result`;
- a `cloud` session stays `running`, and its worker's results show as
  `agent.tool_result`.

A roster thread's or the grader's built-in calls show on the session's stream
with their `session_thread_id`, so the official worker runs them from it, and
its results land on the call's thread. A coordinator's `send_to_agent` opens a
roster thread that runs in later steps. An outcome is graded on the grader's
own thread.

The loop calls the model itself, with one key per provider from the
environment: `AGENTD_ANTHROPIC_API_KEY` and `AGENTD_OPENROUTER_API_KEY`. A
model id with a `/` goes to the
OpenAI-compatible API at `AGENTD_MODEL_BASE_URL` (default
`https://openrouter.ai/api/v1`), and any other id to the
Messages API at `AGENTD_ANTHROPIC_BASE_URL`. A Messages API call asks for
thinking: adaptive, or a budget of a quarter of the output cap for
`claude-haiku-4-5`, `claude-sonnet-4-5` and `claude-opus-4-5`. Each call that
reasons writes `agent.thinking` (no content, as live), and its signed thinking
blocks are kept beside that event, never served, and sent back unmodified in
the call that follows. The system prompt and the tool
list carry prompt-cache marks. A call that fails with 408, 409, 429 or 5xx is
retried with backoff and jitter, never sooner than `retry-after`, within the
step's 10 minute deadline.

| Variable | Default | Meaning |
|---|---|---|
| `AGENTD_ROLE` | `all` | `api` serves the API and runs no loop worker; `loop` runs loop workers only and serves no route (each answers `404`); `all` does both. Retention runs in `api` and `all`; every role ticks the scheduler |
| `AGENTD_STEPS_PER_WORKER` | `256` | Steps one replica runs at once |
| `AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE` | `32` | Steps of one workspace held at once |
| `AGENTD_MODEL_ATTEMPTS` | `5` | Tries per model call |
| `AGENTD_MAX_OUTPUT_TOKENS` | `16000` | The output cap per model call |
| `AGENTD_MODEL_RPM`, `AGENTD_MODEL_ITPM` | unset (no limit) | Requests and input tokens per provider and minute, shared by every replica in Valkey (`AGENTD_VALKEY_URL`); a step over it waits for the next minute. Without Valkey there is no limit |
| `AGENTD_MODELS` | unset | Model ids agents may name beyond the built-in Anthropic ones, comma-separated |
| `AGENTD_ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | The Messages API the loop calls |
| `AGENTD_MODEL_BASE_URL` | `https://openrouter.ai/api/v1` | The OpenAI-compatible API the loop calls for a model id with a `/` |
| `AGENTD_COMPACTION_CLEAR_AT` | `100000` | The prompt size, in tokens, past which old tool results are cleared; half the model's window when that is lower |
| `AGENTD_COMPACTION_KEEP_RESULTS` | `3` | The newest tool results kept whole when clearing, from 3 to 5 |
| `AGENTD_TOOL_OUTPUT_TOKEN_LIMIT` | `16000` | The most of one tool result a prompt carries; the stored event stays whole |

Each model's context window comes from its provider's model list, read once
per process: the Messages API's `GET /v1/models/{id}` (`max_input_tokens`) and
the OpenAI-compatible list's `context_length`. A model its provider does not
list fails its step as a model error. The loop estimates a thread's prompt from
its last call's tokens and a token per 4 bytes since. Every tool result is
capped at `AGENTD_TOOL_OUTPUT_TOKEN_LIMIT`. Past the clear threshold, tool
results older than the newest `AGENTD_COMPACTION_KEEP_RESULTS` become
`[tool result cleared: event <id>]`; inputs and error results stay. Past 80% of
the window, and when no tool call is waiting, the step runs a compaction call
instead of the turn, with the turn's system prompt and tools: on-demand
compaction (`compact-2026-09-04`) for an Anthropic model, a handoff prompt for
any other. It summarises from the thread's latest checkpoint, keeps the last
two calls verbatim and quotes the user's messages newest first. Its
`agent.thread_context_compacted` event is the thread's checkpoint; the summary
is kept beside it and never served. Every later call starts with the signed
block, sent back unmodified, or with the summary. A compaction that returns
no summary lets the turn run on the cleared prompt.

### MCP servers

The loop connects to an agent's MCP servers itself, over streamable HTTP, and
never from a worker. Each step reads the session's environment, its stored
connections (`mcp_connections`) and its vault credentials once, and connects
only a server that needs it: at the start of each turn, with no stored
connection, after its credential changed, or when the server answers 404 to the
stored `Mcp-Session-Id`. A server is reached only at a public address: a URL
that resolves to a loopback, private or link-local address, or a host the
environment's `limited` networking does not allow, is not called. A server that
cannot be reached gets `mcp_connection_failed_error` (a refused credential,
`mcp_authentication_failed_error`) before the turn's model call; the turn goes
on without its tools, and the next turn tries again. The credential is the
first of the session's vaults' `mcp_oauth` or `static_bearer` credentials
keyed by the server's URL; with none the connection is unauthenticated. A
request the server answers 401 to a credential is sent once more with no
`Authorization` header. Each request waits up to 125 s for its answer
(`mcp_oauth_validate`'s probe, 30 s). A call that fails gets an `is_error`
`agent.mcp_tool_result` and no `session.error`, and the server stays usable for
the turn's next call: `upstream server error (HTTP <code>)` for an error
status, `upstream server error (HTTP 502)` for a call left unanswered or
unreached, and `MCP server '<name>' authentication failed: …` when the server
refuses a `static_bearer` credential. A step's calls run one after another
under one lease, so about five unanswered calls in one step outlast it and
another worker sends them again. An
expired `mcp_oauth` token is refreshed by one worker, claimed on the
credential's row; a worker that finds the row updated less than 30 s ago runs
the step again a second later. An allowed call is committed with its step and
called by the next one, with no model call in between; an asked one idles the
session `requires_action`. A deployment's fire whose agent names an MCP host
its environment blocks creates no session: its run records
`mcp_egress_blocked_error` and the deployment pauses.

A session's list cost adds each call's provider-reported cost, rounded up to
the cent so a budget is never passed by rounding. A session whose budget is
spent, or that ran a model with no list price, stops its turn with
`budget_reached` before its next call.

## S3 credentials

S3 credentials always come from the AWS default chain: `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY`, a profile (including SSO and `credential_process`),
web identity, the ECS task role or the EC2 instance role. This holds with or without `AGENTD_AWS_ENDPOINT`. With
`AGENTD_S3_ROLE_ARN`, those credentials assume the role and S3 is called as it.

## Vault credentials

With `AGENTD_SECRETS=aws`, each credential's secret material is one Secrets
Manager secret, `<prefix>/<workspace>/<vault>/<credential>`, created under the
configured KMS key and tagged `workspace`, `vault` and `managed-by=agentd`.
Each part of the name is checked against the id charset before any call, so
no input names a secret outside the prefix or in another workspace. A write
creates the secret, or puts a new value when it exists, with one
`ClientRequestToken`, so a retry never adds a second version. A secret deleted
inside its recovery window is restored and written again. Archiving a
credential deletes its secret with the recovery window. Archiving a vault
archives its credentials, and deleting one deletes them, in one transaction
under the vault's row lock; their secrets are deleted right after the commit.
A create or rotation writes its secret first and takes it back out when the
vault or credential turns out archived or gone, so no archived credential
keeps a secret. Reads are not cached.
An `environment_variable` credential's secret is also tagged
`credential_type=environment_variable` when it is created, so that a sandbox
worker's egress swap (`agentd-worker`) is granted those secrets and no others; `agentd vault-secrets
tag` tags the ones created before (1 Postgres read, then one `TagResource`
each; tagging one twice changes nothing).

| Variable | Default | Meaning |
|---|---|---|
| `AGENTD_SECRETS_KMS_KEY_ID` | required | The customer-managed KMS key: key id, key ARN, alias name or alias ARN. At boot `kms:DescribeKey` must show it reachable, customer managed, enabled and symmetric, or agentd does not start. The `aws/secretsmanager` key is refused |
| `AGENTD_SECRETS_PREFIX` | `agentd` | The first segments of every secret name: `/`-separated segments of ASCII letters, digits and `_`, at most 128 characters |
| `AGENTD_SECRETS_REGION` | `AWS_REGION`, `AWS_DEFAULT_REGION` or the profile's, else `us-east-1` | Region |
| `AGENTD_SECRETS_ENDPOINT` | unset (AWS) | Custom endpoint for Secrets Manager and KMS (LocalStack), under the same rule as `AGENTD_AWS_ENDPOINT` |
| `AGENTD_SECRETS_ROLE_ARN` | unset | Role to assume with the default-chain credentials |
| `AGENTD_SECRETS_RECOVERY_DAYS` | `7` | Days a deleted secret stays recoverable, 7 to 30 |

Credentials come from the AWS default chain, as for S3. The least-privilege
IAM policies for agentd, the key and the workers are in
[`deploy/iam`](../deploy/iam/README.md).

Secret values cross the secret store port as `secrecy::SecretString`, whose
`Debug` is redacted and whose memory is zeroed on drop. Errors name the secret
and the AWS error code, never a value or an AWS message. The AWS SDK's
`debug` and `trace` events, which can print request and response bodies, are
dropped from every log whatever `RUST_LOG` says.

## Keys from the environment

agentd reads its own keys from environment variables; how the deployment fills
them (a secrets manager, a task definition) is the deployment's concern. Each
key ring is `<id>:<hex>,…`, ids of letters and digits, keys of at least 16
bytes:

| Variable | What it is |
|---|---|
| `AGENTD_API_KEY_PEPPERS` | The peppers every API key digest is keyed with. The first keys each new digest. A request's key is looked up under every pepper in one query, and a key found under an older one is stored under the first from then on, so a pepper rotates by putting a new one first and dropping the old one once no key is stored under it |
| `AGENTD_WORK_TOKEN_KEYS` | Signs work tokens, `agentd-wt-<key id>.<claims>.<mac>`. A token verifies while its key id is in the ring |
| `AGENTD_PLACEHOLDER_KEYS` | Signs placeholders the same way, `agentd-ph-<key id>.<claims>.<mac>` |
| `AGENTD_RUNTIME_KEYS` | agentd's runtime keys, the current one first. The poller of `cloud` environments polls the runtime queue with the first; every one listed reaches it, so a key rotates by putting the new one first and dropping the old one once no poller uses it. Make one with `printf 'sk-ant-oat01-%sAA' "$(openssl rand 64 \| base64 \| tr '+/' '-_' \| tr -d '=\n')"` |
| `AGENTD_ANTHROPIC_API_KEY`, `AGENTD_OPENROUTER_API_KEY` | The model providers' keys |

Every replica must have the same rings. Secrets Manager holds only tenant vault
credentials.

## Work

Each session has one work item, its id the session's, in the Postgres
`work_items` table, reused for every run. An item waits on a queue: a
`self_hosted` environment's own (its environment id), or the runtime queue
(`runtime`) for `cloud` sessions; it is queued when the session is created.
A worker's poller
polls `GET /v1/environments/{queue}/work/poll` with its environment key and
gets the oldest queued item not handed out in the last `reclaim_older_than_ms`
(default 5000), with its work secret; the item is handed out again if no one
acknowledges it in that time. A heartbeat with no `desired_ttl_seconds`, or 0,
grants a 300 s lease; any other integer is granted and echoed as given. An
item with no heartbeats stays `active`; a `stopping` item whose lease runs out
is served as `stopped`. Archiving or deleting a session stops its item
(`stopping` while a worker holds it) and keeps it. `AGENTD_API_BASE_URL`
(required) is agentd's API as workers reach it, the work secret's
`api_base_url`.

The work secret is base64url JSON with live's nine keys in live's order:
`version` (1), `session_ingress_token` (`""`), `api_base_url`, `sources` and
`auth` (`[]`), `sessions_token` (the item's work token), `claude_code_args`
(`{}`), `environment_variables` and `use_code_sessions` (`true`). For a
`cloud` session, `environment_variables` maps the `secret_name` of each
unarchived `environment_variable` credential in its vaults (the first vault in
`vault_ids` wins a name) to a placeholder, never the value; for a
`self_hosted` session it is `{}`. A placeholder is `agentd-ph-` then
`<key id>.<base64url JSON {session, ws, vault, credential}>.<base64url HMAC-SHA256>`,
signed with the first key of `AGENTD_PLACEHOLDER_KEYS`, so the swap finds the
credential's secret from it and refuses one copied into another session.

A work token reaches its own item (read, ack, heartbeat, stop), its own
session (read, events, stream, and `POST …/events` with `user.tool_result`
and `user.custom_tool_result` for the session's unanswered calls only), the
files its session mounts (read and download, though no API key can download
an upload), the outputs it saves (`POST /v1/files`, scoped to its session and
downloadable, whatever `scope_id` says; it deletes only those), its agents'
skills, and the memory stores its session attaches (a write to one attached
`read_only` is a 403). Anything else does not exist for it. Every token
request reads its item, and a token whose item is `stopped` is a 401.

## API keys

There are three kinds of key, told apart by prefix, and the route decides
which one a request needs: `/v1/organizations` routes take admin keys only,
every other route workspace keys and environment keys, and the work tokens
described under [Work](#work). Every key is 86 base64url characters from 64
random bytes after its prefix, then `AA`. Only an HMAC-SHA256 digest of a key is kept,
keyed with the current pepper of `AGENTD_API_KEY_PEPPERS`, so the stored
digests alone verify nothing. With 512 bits of entropy a key cannot be guessed, so a slow password
hash such as argon2id would only add cost to every request. Nothing is
cached, so a revoke holds from the next request.

- **Workspace keys** (`sk-ant-api03-…`, `apikey_…`) act for one org and one
  workspace. Each is a row in the Postgres `api_keys` table: its digest under
  a unique index, its id, org, workspace, name, partial key hint, and the time
  it was created and revoked. A request computes its key's digest and reads
  the unrevoked row stored under one of its digests.
- **Environment keys** (`sk-ant-oat01-…`, `envkey_…`) are a worker's poller's
  Bearer token, as live issues them in its Console: each acts for one org,
  workspace and `self_hosted` environment, and is a row in the Postgres
  `environment_keys` table, its digest under a unique index. As on live, one
  sent as `x-api-key` is a 401 `invalid x-api-key`; it reaches its
  environment's work list, poll, stats, ack, heartbeat and stop, and the
  reads, events and stream of its environment's sessions (any other
  session is a 404); every other route is live's 403 `permission_error`
  ("OAuth token does not meet scope requirement …"). A workspace key on
  `work/poll` is a 401 `Invalid bearer token`. agentd's own runtime keys
  (`AGENTD_RUNTIME_KEYS`) reach the runtime queue's routes the same
  way, across workspaces, and also `GET /v1/environments/{id}` for an
  environment with a work item on the runtime queue, so a launcher can apply
  its networking; any other environment is the same 403.
- **Admin keys** (`sk-ant-admin01-…`, `adminkey_…`) act for one org. Each is a
  row in the Postgres `admin_keys` table, its digest under a unique index, with
  its id, org, name, partial key hint, and the time it was created and revoked.

```bash
agentd keys create --org <org_id> --workspace <workspace_id> [--name <name>]   # prints the key once, on stdout
agentd keys list                                                              # id, org, workspace, status, hint, created, name
agentd keys revoke <apikey_id>
agentd admin-keys create --org <org_id> [--name <name>]                       # prints the key once, on stdout
agentd admin-keys list                                                        # id, org, status, hint, created, name
agentd admin-keys revoke <adminkey_id>
agentd environment-keys create --org <org_id> --workspace <workspace_id> --environment <env_id> [--name <name>]   # prints the key once
agentd environment-keys list                                                  # id, org, workspace, environment, status, hint, created, name
agentd environment-keys revoke <envkey_id>
agentd vault-secrets tag                                                      # tags existing environment_variable secrets
```

The commands read the same configuration as the server, so `DATABASE_URL` is
required for all of them when `AGENTD_STORE` is unset or `postgres`, and so
is `AGENTD_API_KEY_PEPPERS`. `agentd keys`, `agentd admin-keys` and `agentd
environment-keys` use Postgres (migrated); `agentd vault-secrets tag` reads the
credentials from Postgres and tags their secrets in Secrets Manager. A key is
printed once and never stored; lose it and create another.

## Security

There is no switch that weakens any of these; each is the only behaviour.

- Vault credentials are in Secrets Manager under a customer-managed KMS key.
  There is no plain store.
- API keys are peppered digests in Postgres, made by `agentd keys`, `agentd
  environment-keys` and `agentd admin-keys`. No API key is read from the
  environment and there is no default key; only agentd's own runtime keys are.
- agentd runs no tool and starts no container: a session's tools run on the
  worker that polls its environment, which holds only the work token of the
  item it took.
- An AWS endpoint override is `https://`, or plain `http://` only to the
  machine itself or a single-label container name, as an emulator runs on.
  Such a name must resolve on the private network (a Compose or Kubernetes
  service), never through a DNS search domain to another host.
- `DATABASE_URL` has no default, and the configuration's `Debug` shows it
  without its password.
- agentd serves plain HTTP. Put it behind a TLS-terminating load balancer or
  ingress, keep the listener on a private network, and never expose port 8080
  to the internet: API keys travel in request headers.
- There is no CORS layer, so browsers cannot call agentd across origins, and
  there is no debug or admin endpoint. Every route needs an API key.
- A `cloud` work secret carries a signed placeholder for each
  `environment_variable` credential, never its value.
- The dev and observability stacks require their passwords, publish their
  ports on `127.0.0.1` (OTLP ingest aside), and run Grafana without
  anonymous access.

## Scheduled work

A resource that owns scheduled work (deployments) registers a fire handler
for its kind. The dreams module is not assembled, so no dream handler is
registered. The scheduler delivers each tick to the handler of the
target's kind, which runs it under the tenant that registered the schedule.
A tick whose kind has no handler fails and is logged as dropped.

Schedules go through an outbox. Each change (a deployment's create, pause,
unpause or archive) is written to `schedule_outbox` in the transaction of the
deployment write that causes it, and removed in the same transaction, so the
table stays empty and the change lives on in the WAL. Debezium Server reads
the WAL into `agentd.public.schedule_outbox` of the change feed (below), and
one replica at a time applies the changes in order to the scheduler sink; a change the sink refuses stops the stream until it
applies, and a change that does not decode is logged at `ERROR` and passed
over. The built-in sink is the Postgres scheduler: one `schedules` row per
target, whose due rows every replica claims every 10 s with
`FOR UPDATE SKIP LOCKED`, moving each to its next time and firing it after
the commit. A fire more than a minute late is skipped, so after downtime a
schedule fires again from its next time, with no backfill. Another sink (a
Temporal worker, say) reads the same topic and delivers its fires to
`agentd.fires` of the feed: on Kafka one record per fire, its value
`{"spec": …, "scheduled_at": …}`; on Valkey Streams one entry per fire with
the fields `spec` (the schedule's spec as JSON) and `scheduled_at` (RFC 3339).

Every fire, from either source, passes the fire inbox, `fire_claims`, before
its handler runs. A delivery may repeat (a scheduler retry, a stream entry a
stopped replica left pending), so the first delivery claims the fire by its
target and time, and the others find it held or done. The owner holds the
fire for 15 minutes; a fire not done by then is taken over by the next
delivery, up to 3 runs, after which it is dropped and logged at `ERROR`. A
deployment fire derives its run and session ids from the deployment and the
fire time, so a run taken over finds the session and run the first made and
makes no second one. A stream entry is acknowledged once its fire ran or
needs no run.

## The change feed

Debezium Server reads the committed changes of the tables in `docker/debezium/application.properties` (the
outboxes and the tracked tables of `docs/history.md`) from Postgres's WAL (pgoutput, the publication and slot
`agentd_changes`) and writes each table's to the topic `agentd.public.<table>` of the feed, and the tracked
tables' `agentd-history` delete messages to `agentd.message`. Its offsets are rows of `debezium_offsets` in the
same database, which it creates on first start. Every minute it writes a logical message to the WAL, so the slot
moves on while the captured tables are quiet, and a heartbeat record to `__debezium-heartbeat.agentd`, which agentd
does not read. The same properties
file runs in every deployment; the environment overrides any key in it
(`debezium.source.database.hostname` is `DEBEZIUM_SOURCE_DATABASE_HOSTNAME`).
Compose runs it as the `debezium` service; CI starts it with
`docker/debezium/start-ci.sh`.

The feed is Kafka when `AGENTD_KAFKA_BROKERS` is set, and Debezium runs with
its Kafka sink (the default). Each replica reads each topic in the consumer
group `agentd.<topic>` and commits a record's offset once its handler
succeeds; a record whose handler fails is retried in place, so nothing behind
it in its partition overtakes it. An ordered table's topic has one partition,
which the group gives to one replica at a time. An external scheduler writes
its fires to the topic `agentd.fires`, each record's value
`{"spec": …, "scheduled_at": "<RFC 3339>"}`. The topics' retention trims them.

Without Kafka, the feed is Valkey Streams on `AGENTD_VALKEY_URL`, and
Debezium runs with `DEBEZIUM_SINK_TYPE=redis`. Each replica reads each stream
in the consumer group `agentd`; an ordered stream is read by one replica at a
time under a lease in Valkey. Heartbeats go to the stream
`__debezium-heartbeat.agentd` (the sink stalls on a heartbeat it is told to
skip). An entry stays in its stream after it is acknowledged, so once a minute
each replica trims each stream it reads below the oldest entry a group still
needs (its oldest pending entry, else the last one it was delivered), and the
heartbeat stream to its last 100 entries. Compose and the CI test job run the
Kafka feed; the CI SDK job runs the Valkey Streams feed on a one-node Valkey
cluster.

In production:

- **Postgres** runs with `wal_level=logical` (on RDS and Aurora, the parameter
  `rds.logical_replication=1`, then a reboot) and `max_slot_wal_keep_size` sized as `docs/history.md` says
  ("WAL sizing"), so a stalled slot cannot fill the disk. A slot past that size is invalidated and the changes
  in it are lost, to the feed's handlers and to history.
- **Debezium's role** has `REPLICATION` (on RDS, `GRANT rds_replication`),
  `SELECT` on the captured tables, and `CREATE` on the schema for its offsets
  table. The publication is made once by the tables' owner,
  `CREATE PUBLICATION agentd_changes FOR TABLE` with the tables of `table.include.list`, and Debezium
  runs with `DEBEZIUM_SOURCE_PUBLICATION_AUTOCREATE_MODE=disabled`. A table
  added to the feed joins `table.include.list` and the publication
  (`ALTER PUBLICATION agentd_changes ADD TABLE …`).
- **Debezium Server** runs as exactly one instance (`quay.io/debezium/server`,
  the version compose pins), restarted when it stops; a second instance
  cannot take the slot. Its connections come from the environment:
  `DEBEZIUM_SOURCE_DATABASE_HOSTNAME`, `_PORT`, `_DBNAME`, `_USER`,
  `_PASSWORD` and `_SSLMODE` (`require`); the offsets' JDBC URL, user and
  password in `DEBEZIUM_SOURCE_OFFSET_STORAGE_JDBC_CONNECTION_URL`, `_USER`
  and `_PASSWORD`; and the Kafka sink's
  `DEBEZIUM_SINK_KAFKA_PRODUCER_BOOTSTRAP_SERVERS`. It answers
  `GET /q/health` on port 8080.
- **Kafka** has the topics `agentd.public.<table>` for each table of `table.include.list`,
  `agentd.message` and `agentd.fires`
  made before Debezium starts, each with one partition, replication factor 3
  and `min.insync.replicas=2`, so a record Debezium has written survives the
  loss of a broker. Retention is longer than the longest time every replica
  may be down; a record past it is gone before agentd reads it. agentd reaches
  the brokers at `AGENTD_KAFKA_BROKERS`.
- **Valkey** keeps nothing that must outlive it while the feed is on Kafka:
  the signals are delivered at most once and the budget's counters expire
  after two minutes. A standalone Valkey serves, or a cluster's configuration
  endpoint with `AGENTD_VALKEY_CLUSTER=true`; `rediss://` for TLS.

The signals to alarm on, with their thresholds:

| Signal | Read from | Alarm when |
|---|---|---|
| Slot | `agentd.feed.slot.safe_wal`, `agentd.feed.slot.lost` and `agentd.feed.slot.retained_wal` (`docs/history.md`, "The slot") | safe WAL below 25% of `max_slot_wal_keep_size`, lost, or retained WAL growing for 15 minutes |
| Debezium health | `GET /q/health` | down for 2 minutes |
| Feed backlog | Kafka: the lag of the group `agentd.agentd.public.<table>`; Valkey: `lag` and `pending` of the group `agentd` in `XINFO GROUPS agentd.public.<table>` | above 0 for 10 minutes |

## Webhooks

agentd sends each organization's endpoints, from `AGENTD_WEBHOOK_ENDPOINTS`,
live's webhook events: `session.status_run_started`, `session.status_idled`
and `session.status_terminated` when a session's status changes (an archive
terminates the session), the vault and credential lifecycle events
(`vault.created`, `vault.archived` with one `vault_credential.archived` for each
credential it archives, `vault.deleted` with one `vault_credential.deleted` for
each of its credentials, archived ones included, `vault_credential.created`,
`vault_credential.archived`, `vault_credential.deleted`), and
`vault_credential.refresh_failed`. Each delivery is a `webhook_deliveries` row
written in the commit of the change it reports, one per endpoint. The change
feed hands each new row to the sender, which posts live's body
(`{"type": "event", "id": "whe_…", "created_at", "data": {…}}`) signed with
Standard Webhooks (`webhook-id`, `webhook-timestamp`, `webhook-signature`, keyed
with the endpoint's `whsec_` secret) and `user-agent: Claude-Platform`. A
delivery not answered 2xx is sent twice more, 31 s apart, each attempt signed
anew under the same `webhook-id`, then dropped; the row records
`delivered_at` or `dropped_at`. Delivery is at least once, so a receiver
dedupes on `webhook-id`.

## Telemetry

| Variable | Default | Meaning |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset (no export) | OTLP gRPC endpoint for traces, metrics and logs, such as `http://localhost:4317`. The OpenTelemetry SDK reads the other standard `OTEL_*` variables (`OTEL_METRIC_EXPORT_INTERVAL`, `OTEL_EXPORTER_OTLP_HEADERS`) itself |
| `AGENTD_LOG_FORMAT` | `text` | `json` writes one JSON object per log line to stdout |
| `RUST_LOG` | `info` | Log filter, `tracing-subscriber` syntax |

`docs/observability.md` lists every signal.

