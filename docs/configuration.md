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
| API keys | `AGENTD_KEYS` | `aws`: workspace and environment keys in Postgres and admin keys in Secrets Manager, digested with a pepper in Secrets Manager (see [API keys](#api-keys)) |
| Scheduler | `AGENTD_SCHEDULER` | `local`: in-process, single replica |
| Sandbox runtime | `AGENTD_RUNTIME` | `docker`: each session gets one container, `agentd-<session id>`, from `AGENTD_SANDBOX_IMAGE` (default `agentd-sandbox`, built from `docker/sandbox/Dockerfile`) on the daemon `DOCKER_HOST` names (default the local socket). Archive, delete and terminate remove it. Each sandbox sits on an internal network of its own, `agentd-net-<session id>` on a /28 of 10.213.0.0/16, with no route out: only agentd and the egress service share it, so its only way out is the egress proxy and no other sandbox is reachable. agentd and the egress service join each one when `AGENTD_SANDBOX_JOIN` names their containers; otherwise the `agentd-gateway` container joins it and relays only agentd's API port and the egress proxy's port to the host. The sandbox's first process is the session's harness (`python3 -m agentd_harness`), which runs the agent's tools as `nobody`; the container drops every capability but those the worker needs to switch user and own files, with `no-new-privileges`, 1024 processes, `AGENTD_SANDBOX_MEMORY_MIB` (default 2048) of memory and `AGENTD_SANDBOX_MILLICPUS` (default 2000) of CPU |

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `AGENTD_ADDR` | `0.0.0.0:8080` | Listen address |
| `DATABASE_URL` | required | Postgres connection |
| `AGENTD_PG_READ_URL` | unset (the primary) | A Postgres read replica the events list (`GET /v1/sessions/{id}/events` and a thread's events) reads; every write, stream and other read uses `DATABASE_URL`. A list may lag the primary by the replica's delay |
| `AGENTD_VALKEY_URL` | unset (signals stay in the process) | The Valkey (or Redis) every replica signals session changes through, on the `agentd:sessions` pub/sub channel, so a stream on one replica sees events written on another; `rediss://` for TLS |
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
| `RUST_LOG` | `info` | Log level |


## Sandboxes

Each session's agent loop, its harness (`harness/`), runs in the session's
sandbox. It takes the session's work item with the work token agentd starts
the sandbox with, mounts the session's files and its agents' skills, and runs
the agent with harness-sim's mini-swe-agent loop. The agent's tools are the
official SDK's `agent_toolset` (bash, read, write, edit, glob, grep), run as
`nobody` in a child process that never sees the work token; each call is its
own `agent.tool_use` and `agent.tool_result`. A coordinator delegates to its
roster with `send_to_agent`, each delegation a session thread; outcomes are
graded by an independent grader thread. The harness reads the session through
the public API and writes its events, the client events it takes and its
outputs through `/v1/sessions/{id}/harness/*`, which only that session's work
token reaches. agentd keeps the API, the event log and the sandbox's
lifecycle, and runs no agent loop; the sandbox takes no inbound connection.

A session's `budget` is enforced by the harness. It adds up each model
request's list cost (OpenRouter's `usage.cost`) across every thread, starting
from the session's `usage.list_cost`, and reports it to agentd as a
`session.usage` event. When the session has a `budget.max_list_cost`, the
harness starts no further model request once the cost reaches it or once a
model with no list price ran, and the session and its threads go idle with
`stop_reason` `budget_reached`. A session with no budget never stops for cost,
whether or not its models have a list price. A budget change a client makes
while the session runs applies at the next request.

agentd stops a sandbox with SIGTERM and a 10 second grace before it removes
the container. On SIGTERM the harness uploads, one at a time, the files under
`/mnt/session/outputs/` that changed since its last turn ended, then exits.
Saving is best effort: if an upload fails, the harness stops uploading and
exits, so the files already uploaded are saved and the rest are lost. Files
not uploaded when the grace runs out are lost too.

The harness calls the model at an OpenAI-compatible API through the egress
proxy. The key is an operator credential: `agentd model-key set [--host
<host>]` reads it from stdin and stores it in Secrets Manager
(`PREFIX/operator/models/openrouter`), bound to the model host; every
session's registration with the egress service carries it, so the proxy puts
it into the harness's model requests. The sandbox sees only its placeholder,
and the agent's tools not even that.

| Variable | Default | Meaning |
|---|---|---|
| `AGENTD_MODELS` | unset | Model ids agents may name beyond the built-in Anthropic ones, comma-separated |
| `AGENTD_MODEL_BASE_URL` | `https://openrouter.ai/api/v1` | The OpenAI-compatible API the harness calls the model at; its host is reachable under any networking |
| `AGENTD_SANDBOX_API_URL` | `http://<agentd-gateway or the first AGENTD_SANDBOX_JOIN container>:<AGENTD_ADDR port>` | agentd's API as a sandbox's harness reaches it |
| `AGENTD_SANDBOX_JOIN` | unset | agentd's container and the egress service's, comma-separated, which join each sandbox's network in place of the gateway |
| `AGENTD_WORK_TOKEN_KEY` | `<prefix>/system/work_token_key` | Hex key (at least 16 bytes) that signs work tokens and egress proxy passwords in place of the system secret, which is then never read |
| `AGENTD_API_BASE_URL` | `AGENTD_SANDBOX_API_URL` | agentd's API as a worker reaches it: the work secret's `api_base_url` |
| `AGENTD_EGRESS_ADMIN_URL` | unset | The egress service's admin interface; unset, sandboxes reach nothing beyond agentd |
| `AGENTD_EGRESS_ADMIN_TOKEN` | unset | The token that interface takes; set with the URL |
| `AGENTD_EGRESS_ADDR` | `0.0.0.0:8890` | The egress proxy's address; its port is the one the gateway relays |
| `AGENTD_SANDBOX_EGRESS_HOST` | `<agentd-gateway or the second AGENTD_SANDBOX_JOIN container>:<AGENTD_EGRESS_ADDR port>` | The egress proxy as a sandbox reaches it |

### Credentials and egress

Each `environment_variable` credential in the session's vaults is set in the
sandbox under its own name, with a per-session placeholder
(`agentd-ph-...`) as its value; the system prompt lists each name, its
description and its hosts. agentd reads no credential value for this: when
it starts the sandbox, it registers the session with the egress service (its
proxy password digest, its environment's hosts, and each credential's name,
placeholder and vault), and the service answers with the session's CA, which
the sandbox starts with (`AGENTD_EGRESS_CA`).

Sandbox traffic leaves through the egress proxy, which the sandbox reaches
through `HTTP_PROXY` and `HTTPS_PROXY` and trusts through the session CA,
whose key never leaves the service. When a request carries a credential's
placeholder, the service reads the credential's current (`AWSCURRENT`) value
from Secrets Manager for that request only; a value changed in Secrets
Manager applies to the next request, and a read that fails refuses the
request (fail closed). For a host the credential allows, the value replaces
the placeholder in the request headers. A placeholder sent to any other
host, or from a credential that names no host, is refused, as is a host
outside `limited` networking. The value is held in locked memory, which
never reaches swap, and zeroed as soon as the request is sent; the response
is masked against a fingerprint of it (its length, a rolling hash and a
SHA-256 digest), so a value echoed in the response's headers or body comes
back as its placeholder. Each substitution is logged with the session,
credential, host, method and path, never the value.

### Egress service

`agentd-egress` (`docker/egress/Dockerfile`) is the egress proxy as a service
of its own, apart from agentd and from every sandbox. It runs as a non-root
user, on a read-only root filesystem, from a distroless image with no shell.
At startup it turns core dumps off (`RLIMIT_CORE` 0) and, on Linux, makes
itself non-dumpable (`PR_SET_DUMPABLE` 0), so its memory cannot be dumped or
read by another process. It serves no metrics or debug endpoint, and logs no
request header. Its role may only call `secretsmanager:GetSecretValue` on
credential secrets (`deploy/iam/egress-secrets-policy.json`).

| Variable | Default | Meaning |
|---|---|---|
| `AGENTD_EGRESS_ADDR` | `0.0.0.0:8890` | The proxy, for sandboxes |
| `AGENTD_EGRESS_ADMIN_ADDR` | `127.0.0.1:8891` | The admin interface, for agentd only: bind it to an address on a network only agentd shares |
| `AGENTD_EGRESS_ADMIN_TOKEN` | required | The bearer token the admin interface takes, at least 32 characters |
| `AGENTD_EGRESS_INTERNAL_HOSTS` | unset | Hosts sandboxes may reach at internal addresses, comma-separated |
| `AGENTD_SECRETS_PREFIX`, `AGENTD_SECRETS_REGION`, `AGENTD_SECRETS_ENDPOINT`, `AGENTD_SECRETS_ROLE_ARN` | as for agentd | Where the credentials are |

The service keeps its registrations in memory: after it restarts, a sandbox
it no longer knows is refused (407) until the session's sandbox is started
again.

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
`credential_type=environment_variable` when it is created, so that the egress
swap's IAM policy reaches those secrets and no others; `agentd vault-secrets
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
IAM policies for agentd, the key and the sandboxes are in
[`deploy/iam`](../deploy/iam/README.md).

Secret values cross the secret store port as `secrecy::SecretString`, whose
`Debug` is redacted and whose memory is zeroed on drop. Errors name the secret
and the AWS error code, never a value or an AWS message. The AWS SDK's
`debug` and `trace` events, which can print request and response bodies, are
dropped from every log whatever `RUST_LOG` says.

## System secrets

agentd keeps four values once per deployment as `<prefix>/system/<name>`,
under the same KMS key as the credentials, read at boot and created the first
time; another replica, or the next boot, reads the same values:

| Name | What it is |
|---|---|
| `api_key_pepper` | 32 random bytes, hex: keys every API key and environment key digest |
| `work_token_key` | 32 random bytes, hex: signs work tokens, so a token outlives the process that minted it (`AGENTD_WORK_TOKEN_KEY` replaces it) |
| `egress_key` | 32 random bytes, hex: signs `environment_variable` placeholders, which the egress swap checks |
| `runtime_key` | agentd's runtime key, an `sk-ant-oat01-` environment key for the runtime queue |

## Work

Each session has one work item, its id the session's, in the Postgres
`work_items` table, reused for every run. An item waits on a queue: a
`self_hosted` environment's own (its environment id), the runtime queue
(`runtime`) for `cloud` sessions, or the hosted queue of sessions whose own
sandbox's harness takes the item, which no poll hands out. A worker's poller
polls `GET /v1/environments/{queue}/work/poll` with its environment key and
gets the oldest queued item not handed out in the last `reclaim_older_than_ms`
(default 5000), with its work secret; the item is handed out again if no one
acknowledges it in that time. A heartbeat with no `desired_ttl_seconds`, or 0,
grants a 300 s lease; any other integer is granted and echoed as given. An
item with no heartbeats stays `active`; a `stopping` item whose lease runs out
is served as `stopped`. Archiving, deleting or terminating a session stops its
item (`stopping` while a worker holds it) and keeps it.

The work secret is base64url JSON with live's nine keys in live's order:
`version` (1), `session_ingress_token` (`""`), `api_base_url`, `sources` and
`auth` (`[]`), `sessions_token` (the item's work token), `claude_code_args`
(`{}`), `environment_variables` and `use_code_sessions` (`true`). For a
`cloud` session, `environment_variables` maps the `secret_name` of each
unarchived `environment_variable` credential in its vaults (the first vault in
`vault_ids` wins a name) to a placeholder, never the value; for a
`self_hosted` session it is `{}`. A placeholder is `agentd-ph-` then
`<base64url JSON {session, ws, vault, credential}>.<base64url HMAC-SHA256>`,
keyed with HMAC-SHA256(`egress_key`, `"placeholder"`), so the swap finds the
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
keyed with a pepper agentd keeps as `<prefix>/system/api_key_pepper` in
Secrets Manager (created on first use), so the stored digests alone verify
nothing. With 512 bits of entropy a key cannot be guessed, so a slow password
hash such as argon2id would only add cost to every request. Nothing is
cached, so a revoke holds from the next request.

- **Workspace keys** (`sk-ant-api03-…`, `apikey_…`) act for one org and one
  workspace. Each is a row in the Postgres `api_keys` table: its digest under
  a unique index, its id, org, workspace, name, partial key hint, and the time
  it was created and revoked. A request computes its key's digest and reads
  the unrevoked row with that digest. Secrets under
  `<prefix>/system/api_keys/` are never read: reissue each of those keys with
  `agentd keys create`, and switch callers to the new keys before deploying.
- **Environment keys** (`sk-ant-oat01-…`, `envkey_…`) are a worker's poller's
  Bearer token, as live issues them in its Console: each acts for one org,
  workspace and `self_hosted` environment, and is a row in the Postgres
  `environment_keys` table, its digest under a unique index. As on live, one
  sent as `x-api-key` is a 401 `invalid x-api-key`; it reaches its
  environment's work list, poll, stats, ack, heartbeat and stop, and the
  reads, events and stream of its environment's sessions (any other
  session is a 404); every other route is live's 403 `permission_error`
  ("OAuth token does not meet scope requirement …"). A workspace key on
  `work/poll` is a 401 `Invalid bearer token`. agentd's own runtime key
  (`<prefix>/system/runtime_key`) reaches the runtime queue's routes the same
  way, across workspaces, and also `GET /v1/environments/{id}` for an
  environment with a work item on the runtime queue, so a launcher can apply
  its networking; any other environment is the same 403.
- **Admin keys** (`sk-ant-admin01-…`, `adminkey_…`) act for one org. Each is a
  Secrets Manager secret, `<prefix>/system/admin_keys/<digest>`, under the
  same KMS key as the credentials, holding the digest, its id and org; the
  tags carry what a listing shows. A request reads the one secret its digest
  names and compares the digest in constant time. A revoke deletes the secret.

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
required for all of them when `AGENTD_STORE` is unset or `postgres`. `agentd
keys` and `agentd environment-keys` use Postgres (migrated) and Secrets
Manager for the pepper; `agentd vault-secrets tag` reads the credentials from
Postgres and tags their secrets in Secrets Manager; `agentd admin-keys` uses
only Secrets Manager. A key is printed once and never stored;
lose it and create another. No environment variable holds a key.

## Security

There is no switch that weakens any of these; each is the only behaviour.

- Vault credentials are in Secrets Manager under a customer-managed KMS key.
  There is no plain store.
- API keys are peppered digests, workspace and environment keys in Postgres
  and admin keys in Secrets Manager, made by `agentd keys`, `agentd
  environment-keys` and `agentd admin-keys`. No key is
  read from the environment and there is no default key.
- Every session runs in a Docker sandbox; there is no mode without one. Give
  agentd a Docker API that allows only networks, images and containers (the
  dev stack uses `tecnativa/docker-socket-proxy`), never the raw socket. The
  right to create containers is still as strong as root on that daemon's
  host, so in production that daemon runs on a host or VM of its own, with
  nothing else on it.
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
- Credential values reach a sandbox's requests only through the egress
  service, which reads each one when a request needs it and zeroes it once
  the request is sent; agentd never reads them for egress. Only agentd can
  reach the service's admin interface: it takes a token, and listens on a
  network only the two share.
- The dev and observability stacks require their passwords, publish their
  ports on `127.0.0.1` (OTLP ingest aside), and run Grafana without
  anonymous access.

## Scheduled work

A resource that owns scheduled work (deployments) registers a fire handler
for its kind. The dreams module is not assembled, so no dream handler is
registered. The scheduler delivers each tick to the handler of the
target's kind, which runs it under the tenant that registered the schedule.
A tick whose kind has no handler fails and is logged as dropped.

## Telemetry

| Variable | Default | Meaning |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset (no export) | OTLP gRPC endpoint for traces, metrics and logs, such as `http://localhost:4317`. The OpenTelemetry SDK reads the other standard `OTEL_*` variables (`OTEL_METRIC_EXPORT_INTERVAL`, `OTEL_EXPORTER_OTLP_HEADERS`) itself |
| `AGENTD_LOG_FORMAT` | `text` | `json` writes one JSON object per log line to stdout |
| `RUST_LOG` | `info` | Log filter, `tracing-subscriber` syntax |

`docs/observability.md` lists every signal.

