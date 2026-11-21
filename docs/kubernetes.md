# Kubernetes

agentd runs on Kubernetes from the Helm chart in `deploy/helm/agentd`. The chart holds agentd's own workloads
only: the `api` and `loop` replicas and the migrations. Postgres, Valkey, the change feed (Kafka and Debezium
Server) and S3 run outside it, and agentd reaches them through the environment the chart sets. Every setting is
in `configuration.md`; what to run and watch in production is in `operations.md`.

## Quickstart

Build the two images from the same commit, create the Secrets, then install the chart with the backing
services' addresses:

```sh
docker build -f docker/Dockerfile -t <registry>/agentd:<tag> .
docker build -f deploy/migrations.Dockerfile -t <registry>/agentd-migrations:<tag> .

kubectl create secret generic agentd-database --from-literal=username=agentd --from-literal=password=<password>
kubectl create secret generic agentd-keys \
  --from-literal=AGENTD_API_KEY_PEPPERS=k1:$(openssl rand -hex 32) \
  --from-literal=AGENTD_WORK_TOKEN_KEYS=k1:$(openssl rand -hex 32) \
  --from-literal=AGENTD_PLACEHOLDER_KEYS=k1:$(openssl rand -hex 32) \
  --from-literal=AGENTD_SECRET_KEYS=k1:$(openssl rand -hex 32)
kubectl create secret generic agentd-model --from-literal=AGENTD_ANTHROPIC_API_KEY=<key>

helm install agentd deploy/helm/agentd -f my-values.yaml
```

```yaml
# my-values.yaml
image: { repository: <registry>/agentd, tag: <tag> }
migrations:
  image: { repository: <registry>/agentd-migrations, tag: <tag> }
database: { host: <postgres host> }
env:
  AGENTD_API_BASE_URL: https://agentd.example.com
  AGENTD_VALKEY_URL: rediss://<valkey host>:6379
  AGENTD_VALKEY_CLUSTER: "1"
  AGENTD_KAFKA_BROKERS: <broker>:9092
  AGENTD_BLOB_BUCKET: <bucket>
```

`helm install --wait` returns once the migrations Job has run and every replica listens. The chart creates
`agentd-api` (the Service in front of the `api` replicas), `agentd-loop` and the hook Job `agentd-migrations`.

## The chart

| Value | Default | Meaning |
|---|---|---|
| `image.repository`, `image.tag`, `image.digest`, `image.pullPolicy` | `agentd`, `dev`, unset, `IfNotPresent` | agentd's image (`docker/Dockerfile`), pulled by `digest` when one is set, else by `tag` |
| `migrations.image.*` | `agentd-migrations`, `dev`, unset, `IfNotPresent` | the migrations' image, built with the same version as agentd's |
| `roles.api`, `roles.loop` | 2 replicas each, 128 MiB requested, 512 MiB limit | one Deployment per role, with its `replicas` and `resources` |
| `roles.<role>.serviceAccount.annotations`, `migrations.serviceAccount.annotations` | empty | annotations on the role's ServiceAccount (`<release>-<role>`, `<release>-migrations`), for example `eks.amazonaws.com/role-arn` |
| `service.type`, `service.port`, `service.nodePort` | `ClusterIP`, 8080, unset | the Service in front of the `api` replicas |
| `database.host`, `port`, `name` | `postgres`, 5432, `agentd` | where Postgres is |
| `database.sslMode` | `verify-full` | libpq's `sslmode` (`verify-full`, `verify-ca`, `require`, `prefer`, `disable`), for agentd and the migrations |
| `database.caBundle.configMapName`, `key` | unset, `ca.pem` | a ConfigMap, made before the install, holding the CA bundle that signs the server's certificate (for RDS, AWS's regional bundle); agentd gets it as `sslrootcert` and Flyway as its own; unset, the certificate is checked against the operating system's roots (Java's for Flyway) |
| `database.secretName`, `userKey`, `passwordKey` | `agentd-database`, `username`, `password` | the Secret, made before the install (by hand, External Secrets, Sealed Secrets, CloudNativePG), and its keys holding the user and password; the chart only reads it, and the password holds no single quote or backslash |
| `env` | empty | plain settings, in a ConfigMap; `AGENTD_API_BASE_URL` and `AGENTD_VALKEY_URL` are required |
| `envFromSecrets` | `agentd-keys`, then `agentd-model` (optional) | Secrets whose every key becomes an environment variable |
| `terminationGracePeriodSeconds`, `preStopSleepSeconds` | 30, 5 | the stop: the Service drops the pod, then SIGTERM drains requests for 10 s |
| `podLabels`, `podAnnotations` | empty | labels and annotations on every pod, merged under a role's own `roles.<role>.podLabels` and `podAnnotations`; the migrations' pod takes the annotations |
| `nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints`, `priorityClassName` | empty | where every pod runs; a role (`roles.<role>.nodeSelector`, ...) or `migrations` that sets one replaces it; see "Scheduling" below |
| `networkPolicy.enabled`, `ingressFrom`, `servicePorts`, `https`, `extraEgress` | off; any source; the backing services' ports; 443 outside the private ranges; none | a NetworkPolicy over the release's pods; see "Network policy" below |

Secrets reach agentd only as environment variables from Kubernetes Secrets the chart names; the chart never
holds a secret value. `agentd-database` holds the database user and password; agentd gets them as `DATABASE_URL` in libpq's `key=value` form, and Flyway as its own URL, user and password. `agentd-keys` holds the four key rings (`AGENTD_API_KEY_PEPPERS`, `AGENTD_WORK_TOKEN_KEYS`,
`AGENTD_PLACEHOLDER_KEYS`, `AGENTD_SECRET_KEYS`), the same for every replica. `agentd-model` holds
`AGENTD_ANTHROPIC_API_KEY` or `AGENTD_OPENROUTER_API_KEY`; without it agentd serves the API and every turn ends
on a `session.error` naming the missing provider.

Probes are TCP on 8080: agentd, like live, has no health route. Every pod, the migrations' included, runs as uid
10001 with a read-only root filesystem, no capabilities and no Kubernetes API token. The CLI (`agentd keys create` and the others) reads the same environment as the
server, so it runs in an api pod:

```sh
kubectl exec deploy/agentd-api -- agentd keys create --org <org_id> --workspace <workspace_id> --name <name>
```

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

## Network policy

With `networkPolicy.enabled`, the release's pods take traffic only on 8080 (from `ingressFrom`, any source when
empty) and send it only to DNS, the backing services' ports (`servicePorts`) and HTTPS outside the private ranges
(`https`). That keeps them from the cloud metadata address and from other services in the cluster. AWS's
endpoints inside a VPC (S3, Secrets Manager, KMS) are private addresses, so they need a rule in `extraEgress`:

```yaml
networkPolicy:
  enabled: true
  ingressFrom:
    - namespaceSelector: { matchLabels: { kubernetes.io/metadata.name: ingress-nginx } }
  extraEgress:
    - to: [{ ipBlock: { cidr: 10.0.0.0/16 } }]
      ports: [{ port: 443 }]
```

The cluster's network plugin must enforce NetworkPolicy for it to have any effect.

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
- `AGENTD_API_BASE_URL` is agentd's address as the customers' workers reach it, behind TLS. The Service serves
  plain HTTP on 8080, so TLS ends in front of it, at an ingress or a load balancer, and a service mesh with mutual
  TLS covers the hop from there to the pods.
- `database.caBundle` names the RDS CA bundle, so `verify-full` checks the database's certificate.
- The Secrets come from the deployment's own store (External Secrets, Sealed Secrets, a vault); agentd reads only
  the environment.
