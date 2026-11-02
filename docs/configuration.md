# Configuration

The binary reads its configuration from the environment once, at boot
(`api::Config::from_lookup`). Nothing else in agentd reads the environment,
except the OpenTelemetry SDK, which reads the standard `OTEL_*` variables.
Every port gets one backend, chosen by a selector variable; an unknown value
stops the boot with an error naming the variable.

## Backends

| Port | Selector | Values (default first) |
|---|---|---|
| Record store | `AGENTD_STORE` | `postgres` |
| Blob store | `AGENTD_BLOBS` | `s3` |
| Secret store | `AGENTD_SECRETS` | `blob`: secrets as objects in the blob store, not encrypted |
| API keys | `AGENTD_KEYS` | `env`: the keys in `AGENTD_API_KEYS` |
| Scheduler | `AGENTD_SCHEDULER` | `local`: in-process, single replica |
| Sandbox runtime | `AGENTD_RUNTIME` | `none`: sessions run without a sandbox. `docker`: each session gets one container, `agentd-<session id>`, from `AGENTD_SANDBOX_IMAGE` (default `agentd-sandbox`, built from `docker/sandbox/Dockerfile`) on the daemon `DOCKER_HOST` names (default the local socket). Archive, delete and terminate remove it. Every sandbox sits on the internal `agentd-sandboxes` network, which has no route out and container-to-container traffic off: its only way out is agentd's egress proxy, which applies the environment's networking. agentd reaches that network by joining it when `AGENTD_SANDBOX_JOIN` names agentd's own container, else through the `agentd-gateway` container, which relays only agentd's API and proxy ports to the host. The sandbox's first process is `agentd-worker`, which runs each command as `nobody`; the container drops every capability but those the worker needs to switch user and own files, with `no-new-privileges`, 1024 processes, `AGENTD_SANDBOX_MEMORY_MIB` (default 2048) of memory and `AGENTD_SANDBOX_MILLICPUS` (default 2000) of CPU |

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `AGENTD_ADDR` | `0.0.0.0:8080` | Listen address |
| `DATABASE_URL` | `postgres://agentd:agentd@localhost:5432/agentd` | Postgres connection |
| `AGENTD_DB_POOL_SIZE` | `10` | Postgres connections per replica |
| `AGENTD_DB_POOL_WAIT_MS` | `5000` | Longest wait for a free connection, and for opening or recycling one; past it the request fails instead of queueing |
| `AGENTD_BLOB_BUCKET` | `agentd-artifacts` | The bucket every blob lives in |
| `AGENTD_S3_REGION` | `AWS_REGION`, `AWS_DEFAULT_REGION` or the profile's, else `us-east-1` | Bucket region |
| `AGENTD_AWS_ENDPOINT` | unset (AWS S3) | Custom S3 endpoint (MinIO, LocalStack) |
| `AGENTD_S3_PATH_STYLE` | `1` with a custom endpoint, else `0` | Path-style bucket addressing |
| `AGENTD_S3_ROLE_ARN` | unset | Role to assume with the default-chain credentials |
| `AGENTD_S3_CREATE_BUCKET` | `0` | Create the bucket at boot when it is missing |
| `AGENTD_S3_LOCATION_CONSTRAINT` | the region on AWS S3 (none in `us-east-1`); none with a custom endpoint | The location constraint a created bucket gets, for an S3-compatible store that names locations its own way |
| `AGENTD_API_KEYS` | unset | `key:org:workspace[,key:org:workspace…]` |
| `AGENTD_DEV_KEY` | `0` | `1` also registers `agentd-dev-key` for `org_dev`/`wrkspc_01dev` |
| `RUST_LOG` | `info` | Log level |

At least one API key is required: with neither `AGENTD_API_KEYS` nor
`AGENTD_DEV_KEY=1`, agentd does not start.

## Agent loop

The agent loop runs whenever the sandbox runtime is `docker`. It calls the
model from agentd and runs each tool call in the session's sandbox, whose
`agentd-worker` long-polls agentd for work; the sandbox takes no inbound
connection.

| Variable | Default | Meaning |
|---|---|---|
| `AGENTD_ANTHROPIC_API_KEY` | unset | Key for `claude-*` models |
| `AGENTD_ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Anthropic API base |
| `AGENTD_OPENAI_API_KEY` | unset | Key for `vendor/model` ids, over the OpenAI-compatible API |
| `AGENTD_OPENAI_BASE_URL` | `https://openrouter.ai/api/v1` | OpenAI-compatible API base |
| `AGENTD_MOCK_MODEL` | unset | Path to a per-agent script that answers every model request instead (tests, demos) |
| `AGENTD_MODELS` | unset | Model ids agents may name beyond the built-in Anthropic ones, comma-separated |
| `AGENTD_MAX_OUTPUT_TOKENS` | `16000` | Output tokens per model call |
| `AGENTD_BRAVE_API_KEY` | unset | Brave Search key for `web_search` |
| `AGENTD_MAX_ACTIVE_SESSIONS` | `64` | Session loops one replica runs at once, each holding a lease connection |
| `AGENTD_SANDBOX_API_URL` | `http://<agentd-gateway or AGENTD_SANDBOX_JOIN>:<AGENTD_ADDR port>` | agentd's API as a sandbox's worker reaches it |
| `AGENTD_SANDBOX_JOIN` | unset | agentd's own container, which joins the sandbox network in place of the gateway |
| `AGENTD_WORK_TOKEN_KEY` | random per process | Hex key that signs work tokens and egress proxy passwords; set it when more than one replica runs |
| `AGENTD_EGRESS_ADDR` | `0.0.0.0:8890` | Where the egress proxy listens |
| `AGENTD_SANDBOX_EGRESS_HOST` | `<agentd-gateway or AGENTD_SANDBOX_JOIN>:<AGENTD_EGRESS_ADDR port>` | The egress proxy as a sandbox reaches it |
| `AGENTD_EGRESS_INTERNAL_HOSTS` | unset | Hosts sandboxes may reach at internal addresses, comma-separated |

### Credentials and egress

Each `environment_variable` credential in the session's vaults is set in the
sandbox under its own name, with a per-session placeholder
(`agentd-ph-...`) as its value; the system prompt lists each name, its
description and its hosts. Sandbox traffic leaves through the egress proxy,
which the sandbox reaches through `HTTP_PROXY` and `HTTPS_PROXY` and trusts
through a per-session CA. For a host the credential allows, the proxy puts
the real value in place of the placeholder in the request headers. A
placeholder sent to any other host, or from a credential that names no host,
is refused, as is a host outside `limited` networking. A real value in a
response, headers or body, is masked back to its placeholder. Each
substitution is logged with the session, credential, host, method and path,
never the value. The proxy reads a session's values again at most 30 seconds
after it last read them, so a rotated secret takes effect without a restart.

## S3 credentials

S3 credentials always come from the AWS default chain: `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY`, a profile (including SSO and `credential_process`),
web identity, the ECS task role or the EC2 instance role. This holds with or without `AGENTD_AWS_ENDPOINT`. With
`AGENTD_S3_ROLE_ARN`, those credentials assume the role and S3 is called as it.

## Scheduled work

A resource that owns scheduled work (deployments, dreams) registers a fire
handler for its kind. The scheduler delivers each tick to the handler of the
target's kind, which runs it under the tenant that registered the schedule.
A tick whose kind has no handler fails and is logged as dropped.

## Telemetry

| Variable | Default | Meaning |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset (no export) | OTLP gRPC endpoint for traces, metrics and logs, such as `http://localhost:4317`. The OpenTelemetry SDK reads the other standard `OTEL_*` variables (`OTEL_METRIC_EXPORT_INTERVAL`, `OTEL_EXPORTER_OTLP_HEADERS`) itself |
| `AGENTD_LOG_FORMAT` | `text` | `json` writes one JSON object per log line to stdout |
| `RUST_LOG` | `info` | Log filter, `tracing-subscriber` syntax |

`docs/observability.md` lists every signal.

