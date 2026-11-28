#!/bin/sh
# Build and start agentd with Postgres (records and vault secrets) and LocalStack (S3),
# make an API key through the CLI, run the k6 suite, and leave the summary in
# perf/results/summary.json. Each run gets a fresh database password and key.
#   perf/run.sh                 the suite at the defaults (RATE=20, STREAMS=50, DURATION=2m)
#   RATE=50 perf/run.sh         heavier load
#   perf/run.sh --observe       also export telemetry to docker/observability (start it first)
#   perf/run.sh --down          stop and remove the stack and its volumes
#   THRESHOLDS=thresholds.smoke.json perf/run.sh   only the failure-rate thresholds
set -eu
cd "$(dirname "$0")/.."
# PERF_PROJECT names the Compose project (default agentd-perf), so two stacks can coexist.
compose() { docker compose -p "${PERF_PROJECT:-agentd-perf}" --project-directory docker -f docker/docker-compose.yml -f perf/compose.yaml "$@"; }
case "${1:-}" in
  --down)
    export AGENTD_API_KEY_PEPPERS=unset AGENTD_WORK_TOKEN_KEYS=unset AGENTD_PLACEHOLDER_KEYS=unset AGENTD_SECRET_KEYS=unset
    POSTGRES_PASSWORD=unset API_KEY=unset compose --profile run down -v; exit 0 ;;
  --observe) export OTEL_EXPORTER_OTLP_ENDPOINT=http://host.docker.internal:4317 ;;
esac
mkdir -p perf/results
POSTGRES_PASSWORD=$(openssl rand -hex 16)
API_KEY=unset
# Its own container name, so it runs beside the dev stack.
AGENTD_CONTAINER=agentd-perf
export POSTGRES_PASSWORD API_KEY AGENTD_CONTAINER
# Disposable key rings, the vault secrets' master keys among them: each run starts from empty volumes.
for v in AGENTD_API_KEY_PEPPERS AGENTD_WORK_TOKEN_KEYS AGENTD_PLACEHOLDER_KEYS AGENTD_SECRET_KEYS; do
  export "$v=k1:$(openssl rand -hex 32)"
done
# Each run starts from empty volumes, so runs are comparable.
compose --profile run down -v >/dev/null 2>&1 || true
compose up -d --build --wait agentd
compose run --rm --no-deps -T agentd organization create --id org_perf >/dev/null
API_KEY=$(compose run --rm --no-deps -T agentd keys create --org org_perf --workspace wrkspc_perf --name perf)
compose --profile run run --rm k6
