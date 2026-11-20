#!/bin/sh
# Bring agentd up on a local kind cluster: the backing services of deploy/kind/deps, the chart
# deploy/helm/agentd (migrations, two api and two loop replicas), and Debezium Server. agentd answers at
# http://127.0.0.1:8080 and the script prints a workspace key. Run again to rebuild and roll out a change.
#   deploy/kind/up.sh
# The cluster holds no model key: deploy/kind/e2e.sh adds one for its run and removes it after.
set -eu
cd "$(dirname "$0")/../.."
. deploy/kind/lib.sh

kind get clusters 2>/dev/null | grep -qx "$CLUSTER" \
  || kind create cluster --name "$CLUSTER" --config deploy/kind/cluster.yaml
docker build -q -f docker/Dockerfile -t agentd:kind . >/dev/null
docker build -q -f deploy/migrations.Dockerfile -t agentd-migrations:kind . >/dev/null
docker build -q -f deploy/kind/ant-worker.Dockerfile -t agentd-ant-worker:kind deploy/kind >/dev/null
kind load docker-image --name "$CLUSTER" agentd:kind agentd-migrations:kind agentd-ant-worker:kind

kubectl --context "$CTX" create namespace "$NS" --dry-run=client -o yaml | kubectl --context "$CTX" apply -f - >/dev/null
# Made once per cluster: the keys and sealed vault credentials made under them stay valid across runs.
k get secret agentd-database >/dev/null 2>&1 \
  || printf 'username=agentd\npassword=%s\n' "$(openssl rand -hex 16)" | secret_from_stdin agentd-database
if ! k get secret agentd-keys >/dev/null 2>&1; then
  for v in AGENTD_API_KEY_PEPPERS AGENTD_WORK_TOKEN_KEYS AGENTD_PLACEHOLDER_KEYS AGENTD_SECRET_KEYS; do
    printf '%s=k1:%s\n' "$v" "$(openssl rand -hex 32)"
  done | secret_from_stdin agentd-keys
fi

k apply -f deploy/kind/deps >/dev/null
for s in statefulset/postgres statefulset/valkey statefulset/kafka deployment/localstack; do
  k rollout status "$s" --timeout=10m
done

upgrade=$(helm status agentd --kube-context "$CTX" -n "$NS" >/dev/null 2>&1 && echo yes || true)
helm upgrade --install agentd deploy/helm/agentd --kube-context "$CTX" -n "$NS" \
  -f deploy/kind/values.yaml --wait --timeout 10m >/dev/null
# The images keep their tag across builds, so a rebuild rolls out only on a restart.
if [ -n "$upgrade" ]; then
  k rollout restart deployment/agentd-api deployment/agentd-loop >/dev/null
  k rollout status deployment/agentd-api --timeout=5m
  k rollout status deployment/agentd-loop --timeout=5m
fi

k create configmap debezium-config --from-file=docker/debezium/application.properties \
  --dry-run=client -o yaml | k apply -f - >/dev/null
k apply -f deploy/kind/debezium.yaml >/dev/null
# Debezium Server reads its properties once, at start: their checksum on the pod template rolls it out
# whenever they differ from what it runs, also on a rerun after an interrupted one.
sum=$(shasum -a 256 docker/debezium/application.properties | cut -c1-16)
k patch deployment debezium --type merge \
  -p "{\"spec\":{\"template\":{\"metadata\":{\"annotations\":{\"agentd/config-sha256\":\"$sum\"}}}}}" >/dev/null
k rollout status deployment/debezium --timeout=10m

echo "agentd is up at $BASE_URL. A workspace key (org_dev, wrkspc_01dev):"
cli_key keys create --org org_dev --workspace wrkspc_01dev --name dev
