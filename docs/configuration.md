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

Nothing is held in a replica between steps. A step cut off by a crash runs
again from the log once its lease runs out. A `user.interrupt` sent during a
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
| `AGENTD_ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | The Messages API the loop calls |
| `AGENTD_MODEL_BASE_URL` | `https://openrouter.ai/api/v1` | The OpenAI-compatible API the loop calls for a model id with a `/` |

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
credential deletes its secret with the recovery window. Reads are not cached.
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
the WAL into the Valkey stream `agentd.public.schedule_outbox`, and one
replica at a time, holding a lease in Valkey, applies the changes in order to
the scheduler sink; a change the sink refuses stops the stream until it
applies. The built-in sink is the Postgres scheduler: one `schedules` row per
target, whose due rows every replica claims every 10 s with
`FOR UPDATE SKIP LOCKED`, moving each to its next time and firing it after
the commit. A fire more than a minute late is skipped, so after downtime a
schedule fires again from its next time, with no backfill. Another sink (a
Temporal worker, say) reads the same stream and delivers its fires to the
Valkey stream `agentd.fires`, one entry per fire with the fields `spec` (the
schedule's spec as JSON) and `scheduled_at` (RFC 3339).

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

## Telemetry

| Variable | Default | Meaning |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset (no export) | OTLP gRPC endpoint for traces, metrics and logs, such as `http://localhost:4317`. The OpenTelemetry SDK reads the other standard `OTEL_*` variables (`OTEL_METRIC_EXPORT_INTERVAL`, `OTEL_EXPORTER_OTLP_HEADERS`) itself |
| `AGENTD_LOG_FORMAT` | `text` | `json` writes one JSON object per log line to stdout |
| `RUST_LOG` | `info` | Log filter, `tracing-subscriber` syntax |

`docs/observability.md` lists every signal.

