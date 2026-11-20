#!/bin/sh
# The free checks against agentd on kind: health, the Python, TypeScript and Java SDK conformance suites, and
# the k6 load (perf/agentd.js) from inside the cluster. They send user messages to agents, so they refuse to
# run while the cluster holds a model key: with one, those turns would call the paid model.
#   deploy/kind/check.sh                  smoke load: RATE=10 STREAMS=10 DURATION=30s, thresholds.smoke.json
#   RATE=20 STREAMS=50 DURATION=2m THRESHOLDS=thresholds.json deploy/kind/check.sh
# The suites' worker cases stay skipped: they need the scripted model (conformance/README.md). PYTHON names a
# Python 3.12 (as CI) when `python3` is older.
set -eu
cd "$(dirname "$0")/../.."
. deploy/kind/lib.sh

if k get secret agentd-model >/dev/null 2>&1; then
  echo "the cluster holds a model key (Secret agentd-model): delete it before the free checks" >&2
  exit 1
fi
# A running container keeps the environment it started with, so a deleted Secret's key stays until a restart.
pods=$(k get pods -l app.kubernetes.io/instance=agentd -o name)
[ -n "$pods" ] || { echo "no agentd pod is running: deploy/kind/up.sh first" >&2; exit 1; }
for pod in $pods; do
  if ! k exec "$pod" -- sh -c '[ -z "${AGENTD_ANTHROPIC_API_KEY:-}${AGENTD_OPENROUTER_API_KEY:-}" ]'; then
    echo "$pod still holds a model key: kubectl --context $CTX -n $NS rollout restart deployment/agentd-api deployment/agentd-loop" >&2
    exit 1
  fi
done

echo "== health"
k wait --for=condition=Ready pod -l 'app.kubernetes.io/instance=agentd' --timeout=5m >/dev/null
k wait --for=condition=Ready pod -l 'app in (postgres,valkey,kafka,localstack,debezium)' --timeout=5m >/dev/null
KEY=$(cli_key keys create --org org_sdk --workspace wrkspc_sdk --name check)
ADMIN=$(cli_key admin-keys create --org org_sdk --name check)
curl -fsS -o /dev/null -H "x-api-key: $KEY" -H "anthropic-version: 2023-06-01" \
  -H "anthropic-beta: managed-agents-2026-04-01" "$BASE_URL/v1/agents?limit=1"
echo "every pod is Ready and the API answers"

echo "== conformance"
export AGENTD_BASE_URL="$BASE_URL" AGENTD_TEST_KEY="$KEY" AGENTD_TEST_ADMIN_KEY="$ADMIN"
venv=$(mktemp -d)
trap 'rm -rf "$venv"' EXIT
"${PYTHON:-python3}" -m venv "$venv"
"$venv/bin/pip" install -q -r conformance/python/requirements.txt
(cd conformance/python && "$venv/bin/pytest" -q)
(cd conformance/typescript && npm ci --silent && npm test)
(cd conformance/java && ./gradlew test --no-daemon -q)

echo "== load"
PERF_KEY=$(cli_key keys create --org org_perf --workspace wrkspc_perf --name perf)
printf 'API_KEY=%s\nRATE=%s\nSTREAMS=%s\nDURATION=%s\nTHRESHOLDS=%s\n' "$PERF_KEY" "${RATE:-10}" \
  "${STREAMS:-10}" "${DURATION:-30s}" "${THRESHOLDS:-thresholds.smoke.json}" | secret_from_stdin perf-env
k create configmap perf-scripts --from-file=perf/agentd.js --from-file=perf/thresholds.json \
  --from-file=perf/thresholds.smoke.json --dry-run=client -o yaml | k apply -f - >/dev/null
k create configmap perf-lib --from-file=perf/lib --dry-run=client -o yaml | k apply -f - >/dev/null
k delete job perf --ignore-not-found >/dev/null
k apply -f deploy/kind/perf.yaml >/dev/null
# The Job's activeDeadlineSeconds ends it Failed if it never finishes, an unpulled image included.
state=
until [ -n "$state" ]; do
  sleep 5
  state=$(k get job perf -o jsonpath='{.status.conditions[?(@.status=="True")].type}' | grep -oE 'Complete|Failed' | head -1 || true)
done
k logs job/perf || k describe job perf
[ "$state" = Complete ]
echo "every check passed"
