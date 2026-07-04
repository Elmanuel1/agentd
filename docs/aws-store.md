# AWS-native persistence (agentd-store-aws)

Makes agentd **stateless across replicas** by moving all shared state to AWS:
**DynamoDB** (entities) + **S3** (blobs). Replicas hold nothing; any replica
serves any request; scheduled fires are exactly-once via DynamoDB conditional
writes.

## Local emulator

We test against **Floci** — the token-free, MIT, LocalStack-endpoint-compatible
emulator that replaced LocalStack Community after its March-2026 paywall.

| Emulator | Endpoint | Token? | Notes |
|---|---|---|---|
| **Floci** (default) | `:4566` | no | ~24 ms start, 13 MiB — ideal for CI |
| MiniStack | `:4566` | no | real infra (RDS=Postgres, etc.), 60+ services |
| LocalStack ≥2026.03 | `:4566` | **yes** | needs `LOCALSTACK_AUTH_TOKEN` |

All three speak the same `:4566` API, so agentd's code is emulator-neutral —
only the compose image changes.

```sh
docker run -d --name agentd-floci -p 4566:4566 \
  -v /var/run/docker.sock:/var/run/docker.sock -u root floci/floci:latest
```

## Data model

One DynamoDB table per entity (`{prefix}_{entity}`), `pk` partition + `sk`
sort + `doc` JSON. S3 holds file bytes (and later memory content / workspace
snapshots).

| Pattern | Entity | Layout |
|---|---|---|
| versioned | agents | `pk=workspace`, `sk=agent_id#{version:020}` — latest = max sk |
| exactly-once claim | deployments | `deployment_claims`: `put_if_absent` = the atomic cross-replica boundary |
| append log | deployment runs | `pk=workspace#deployment`, `sk=created_at#run_id` |
| blob | files | metadata in DynamoDB, bytes in S3 `{ws}/{file_id}` |

## Run the integration tests

Hermetic `cargo test` skips them; run against a live emulator explicitly:

```sh
AGENTD_AWS_TESTS=1 AGENTD_AWS_ENDPOINT=http://localhost:4566 \
  AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-east-1 \
  cargo test -p agentd-store-aws --test aws_integration
```

The key test — `dynamo_claim_fire_is_exactly_once_across_concurrent_callers` —
races 8 concurrent callers at one `(deployment, scheduled_at)` and asserts
exactly one wins the DynamoDB conditional write.

## Coverage

Implemented + integration-tested against Floci: **agents** (versioned),
**deployments** (+ claim), **files** (S3). Remaining entities
(environments, vaults, memory, skills, sessions/events, dreams) reuse the
identical `DynamoDoc` mapping — mechanical, one module each.

## Real AWS

Drop `AGENTD_AWS_ENDPOINT`, give the task an IAM role with DynamoDB + S3
permissions. Provision the tables/bucket (the store `ensure_*` calls create
them on first use in dev; use IaC in prod). Pairs naturally with Fargate for
the `Runtime` port — DynamoDB + S3 + Fargate is a fully serverless stack.
