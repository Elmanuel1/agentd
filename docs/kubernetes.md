# Kubernetes

agentd runs on Kubernetes from the Helm chart in `deploy/helm/agentd`. The chart holds agentd's own workloads;
the backing services are connection settings. `deploy/kind` brings the whole system up on a local
[kind](https://kind.sigs.k8s.io) cluster, with the backing services inside it, to test it end to end against the
real model. Every setting is in `configuration.md`; what to run and watch in production is in `operations.md`.

## Quickstart on kind

Needs Docker (the cluster takes about 3 GB of memory), kind 0.24 or later, kubectl, Helm 3, `jq`, and for the checks
Python 3.12 (`PYTHON=python3.12` when `python3` is older), Node and Java 21.

```sh
deploy/kind/up.sh       # cluster, images, backing services, chart, Debezium; prints a workspace key
deploy/kind/check.sh    # the free checks: health, the SDK suites, the k6 smoke load
AGENTD_ANTHROPIC_API_KEY=sk-ant-... deploy/kind/e2e.sh   # the real end-to-end run (paid: one short turn)
deploy/kind/down.sh     # delete the cluster and the images
```

agentd answers at `http://127.0.0.1:8080` (the API Service's NodePort 30080, published by `cluster.yaml`), so the
SDKs, `ant` and `curl` on the host reach it as they reach compose. Port 8080 must be free: stop the compose stack
first. `up.sh` is safe to run again: it rebuilds the images, rolls out the change, and keeps the database
password and key rings it made on the first run. The scripts use the context `kind-agentd` and the namespace
`agentd`, never the current context; `AGENTD_KIND_CLUSTER` names another cluster.

## What runs

| Workload | Kind | From |
|---|---|---|
| `agentd-api` | Deployment, 2 replicas, `AGENTD_ROLE=api`, behind the Service `agentd-api` | the chart |
| `agentd-loop` | Deployment, 2 replicas, `AGENTD_ROLE=loop` | the chart |
| `agentd-migrations` | Job, a Helm `pre-install` and `pre-upgrade` hook: Flyway 11 applies the migrations before agentd rolls out | the chart, image from `deploy/migrations.Dockerfile` |
| `postgres` | StatefulSet, Postgres 16, `wal_level=logical`, `max_slot_wal_keep_size=1GB` | `deploy/kind/deps` |
| `valkey` | StatefulSet, Valkey 9, a one-node cluster with AOF, announced as `valkey` | `deploy/kind/deps` |
| `kafka` | StatefulSet, Kafka 3.9.1, one KRaft node; the change feed | `deploy/kind/deps` |
| `localstack` | Deployment, LocalStack 3.0 with S3 only; the blob store | `deploy/kind/deps` |
| `debezium` | Deployment, 1 replica, `Recreate`: Debezium Server with `docker/debezium/application.properties` | `deploy/kind/debezium.yaml` |
| `ant-worker` | Pod, `ant beta:worker poll` 1.39.0 for `e2e.sh`'s environment | `deploy/kind/worker.yaml`, image from `deploy/kind/ant-worker.Dockerfile` |
| `perf` | Job, k6 1.3.0 running `perf/agentd.js` against `http://agentd-api:8080` | `deploy/kind/perf.yaml` |

Images pinned as compose pins them. The scheduler is the built-in one (`AGENTD_SCHEDULER=postgres`), vault
credentials are sealed in Postgres, and no webhook endpoint is set. The cluster exports no telemetry unless
`OTEL_EXPORTER_OTLP_ENDPOINT` is added to `env`; `docker/observability` on the host is reached at
`http://host.docker.internal:4317`.

## The chart

| Value | Default | Meaning |
|---|---|---|
| `image.repository`, `image.tag`, `image.pullPolicy` | `agentd`, `dev`, `IfNotPresent` | agentd's image (`docker/Dockerfile`) |
| `migrations.image.*` | `agentd-migrations`, `dev`, `IfNotPresent` | the migrations' image, built with the same version as agentd's |
| `roles.api`, `roles.loop` | 2 replicas each, 128 MiB requested, 512 MiB limit | one Deployment per role, with its `replicas` and `resources` |
| `service.type`, `service.port`, `service.nodePort` | `ClusterIP`, 8080, unset | the Service in front of the `api` replicas |
| `database.existingSecret`, `database.urlKey` | `agentd-database`, `DATABASE_URL` | the Secret and its key holding the database URL (`postgres://user:password@host:port/name`, options such as `?sslmode=require` appended), read as `DATABASE_URL` by agentd and split by the migrations into Flyway's JDBC URL, user and password; special characters are URL-encoded, so keep the password URL-safe (hex) |
| `env` | empty | plain settings, in a ConfigMap; `AGENTD_API_BASE_URL` and `AGENTD_VALKEY_URL` are required |
| `envFromSecrets` | `agentd-keys`, then `agentd-model` (optional) | Secrets whose every key becomes an environment variable |
| `terminationGracePeriodSeconds`, `preStopSleepSeconds` | 30, 5 | the stop: the Service drops the pod, then SIGTERM drains requests for 10 s |
| `podLabels`, `podAnnotations` | empty | labels and annotations on every pod, merged under a role's own `roles.<role>.podLabels` and `podAnnotations`; the migrations' pod takes the annotations |
| `nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints`, `priorityClassName` | empty | where every pod runs; a role (`roles.<role>.nodeSelector`, ...) or `migrations` that sets one replaces it; see "Scheduling" below |

Secrets reach agentd only as environment variables from Kubernetes Secrets the chart names; the chart never
holds a secret value. `agentd-database` holds the database URL under `DATABASE_URL`; only that key is read. `agentd-keys` holds the four key rings (`AGENTD_API_KEY_PEPPERS`, `AGENTD_WORK_TOKEN_KEYS`,
`AGENTD_PLACEHOLDER_KEYS`, `AGENTD_SECRET_KEYS`), the same for every replica. `agentd-model` holds
`AGENTD_ANTHROPIC_API_KEY` or `AGENTD_OPENROUTER_API_KEY`; without it agentd serves the API and every turn ends
on a `session.error` naming the missing provider.

Probes are TCP on 8080: agentd, like live, has no health route. Pods run as uid 10001 with a read-only root
filesystem and no capabilities. The CLI (`agentd keys create` and the others) reads the same environment as the
server, so it runs in an api pod:

```sh
kubectl --context kind-agentd -n agentd exec deploy/agentd-api -- agentd keys create --org org_dev --workspace wrkspc_01dev --name dev
```

## The checks

`check.sh` runs the checks that call no model, and refuses to start while the Secret `agentd-model` exists: the
SDK session tests and the load send user messages to agents, which would call the paid model.

1. Health: every pod is Ready, and `GET /v1/agents?limit=1` with a new workspace key answers 200.
2. Conformance: the Python, TypeScript and Java suites at `AGENTD_BASE_URL=http://127.0.0.1:8080`, with a
   workspace key and an admin key for `org_sdk`. Their worker cases stay skipped: they need the scripted model.
3. Load: the k6 Job, at `RATE=10`, `STREAMS=10`, `DURATION=30s` and `thresholds.smoke.json` unless those are set
   in the shell. It runs inside the cluster, so no port-forward sits in the measured path.

`e2e.sh` is the real run: it puts the model key in `agentd-model` and rolls agentd out, makes an agent on
`claude-haiku-4-5` with the built-in tools and a `self_hosted` environment, starts `ant-worker` with an
environment key for it, and sends one message that asks for `echo kind-e2e-ok` through the bash tool. It passes
when `agent.tool_use` and a `user.tool_result` holding `kind-e2e-ok` are in the session's events and the turn
ends with `end_turn`. On exit, pass or fail, it deletes the worker and the model key and rolls agentd out again.

## Scheduling

The chart passes Kubernetes' own pod settings through, so the cluster's node pools and autoscaler (Karpenter,
Cluster Autoscaler, a cloud's own) place agentd by labels, taints and annotations. A role's own setting replaces
the chart-wide one, so the loop can run on spot capacity while the api stays on on-demand nodes:

```yaml
podAnnotations:
  example.com/do-not-disrupt: "true"
nodeSelector:
  node-pool: agentd
roles:
  loop:
    nodeSelector:
      node-pool: agentd
      capacity-type: spot
    tolerations:
      - { key: spot, operator: Exists, effect: NoSchedule }
```

## Argo CD and progressive delivery

The chart renders plain Deployments and holds nothing specific to a deploy tool. An Argo CD `Application` points
at `deploy/helm/agentd` and sets its values in `spec.source.helm`. A canary with Argo Rollouts lives beside it in
the same Application: a `Rollout` with `workloadRef` naming the Deployment `<release>-api` (or `-loop`) runs that
Deployment's pod template through the canary's steps, with its own `replicas`, Services and traffic routing.
The role's `replicas` is 0 in the chart's values, so the Deployment only carries the pod template and Argo CD
keeps it at zero. The chart needs nothing more than its stable Deployment names.

## Production

The chart is the same; the values change:

- `database`, `AGENTD_VALKEY_URL` and `AGENTD_KAFKA_BROKERS` point at managed services (RDS or Aurora,
  ElastiCache, MSK), set up as `configuration.md`'s change feed section says: the publication made by the tables'
  owner, Kafka topics with replication factor 3, and one Debezium Server.
- S3 is AWS's: drop `AGENTD_AWS_ENDPOINT`, `AGENTD_S3_CREATE_BUCKET` and the AWS keys, and give the pods a role
  (EKS Pod Identity or IRSA) with `deploy/iam`'s policies.
- `AGENTD_API_BASE_URL` is agentd's address as the customers' workers reach it, behind TLS: agentd serves plain
  HTTP.
- The Secrets come from the deployment's own store (External Secrets, Sealed Secrets, a vault); agentd reads only
  the environment.
