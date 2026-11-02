#!/bin/sh
# Build and start agentd with Postgres and S3 (LocalStack), wait until it answers, run
# the k6 suite, and leave the summary in perf/results/summary.json.
#   perf/run.sh                 the suite at the defaults (RATE=20, STREAMS=50, DURATION=2m)
#   RATE=50 perf/run.sh         heavier load
#   perf/run.sh --observe       also export telemetry to docker/observability (start it first)
#   perf/run.sh --down          stop and remove the stack and its volumes
set -eu
cd "$(dirname "$0")/.."
compose() { docker compose -p agentd-perf --project-directory docker -f docker/docker-compose.yml -f perf/compose.yaml "$@"; }
case "${1:-}" in
  --down) compose --profile run down -v; exit 0 ;;
  --observe) export OTEL_EXPORTER_OTLP_ENDPOINT=http://host.docker.internal:4317 ;;
esac
mkdir -p perf/results
compose up -d --build agentd
compose --profile run run --rm k6
