#!/bin/sh
# Check that a running agentd reaches the stack in compose.yaml: every metric
# family in Prometheus, traces in Tempo, logs in Loki, and the dashboard and
# alert rules provisioned in Grafana. Run after some traffic.
set -eu
prom() { curl -sf --get http://localhost:9090/api/v1/query --data-urlencode "query=$1" | grep -q '"result":\[{'; }
fail=0
# Each check gets a minute: the collector batches and Tempo ingests late.
check() {
  for _ in 1 2 3 4 5 6; do
    if "$@"; then echo "ok   $label"; return; fi
    sleep 10
  done
  echo "FAIL $label"; fail=1
}
for family in http_server_request_duration_seconds_count http_server_active_requests agentd_sse_active_streams \
  db_client_operation_duration_seconds_count db_client_connection_count db_client_connection_pending_requests \
  agentd_port_operation_duration_seconds_count; do
  label="metric $family"; check prom "$family{job=\"agentd\"}"
done
label="traces in Tempo"
check sh -c 'curl -sf --get http://localhost:3200/api/search --data-urlencode "q={resource.service.name=\"agentd\"}" | grep -q traceID'
label="logs in Loki"
check sh -c 'curl -sf --get http://localhost:3100/loki/api/v1/query_range --data-urlencode "query={service_name=\"agentd\"}" | grep -q "\"values\""'
label="dashboard provisioned"
check sh -c 'curl -sf -u admin:admin http://localhost:3000/api/dashboards/uid/agentd-boundaries >/dev/null'
label="alert rules provisioned"
check sh -c 'test "$(curl -sf -u admin:admin http://localhost:3000/api/v1/provisioning/alert-rules | grep -o "\"uid\":\"agentd-" | wc -l)" -ge 7'
exit $fail
