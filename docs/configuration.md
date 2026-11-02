# Configuration

The binary reads its configuration from the environment once, at boot
(`api::Config::from_lookup`). Nothing else in agentd reads the environment.
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
| Sandbox runtime | `AGENTD_RUNTIME` | `none`: no sandbox, a launch fails |

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
