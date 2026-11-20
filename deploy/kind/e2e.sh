#!/bin/sh
# The real end-to-end run on kind, with the real model: an agent on claude-haiku-4-5 with the built-in tools, a
# self_hosted environment served by the in-cluster worker (deploy/kind/worker.yaml), and one session whose turn
# runs a shell command through the worker. Passes when the tool's result reaches the model and the turn ends.
# Paid: one short turn. The model key is in the cluster only for the run.
#   AGENTD_ANTHROPIC_API_KEY=sk-ant-... deploy/kind/e2e.sh
set -eu
cd "$(dirname "$0")/../.."
. deploy/kind/lib.sh
: "${AGENTD_ANTHROPIC_API_KEY:?set AGENTD_ANTHROPIC_API_KEY to the model key}"

roll() {
  k rollout restart deployment/agentd-api deployment/agentd-loop >/dev/null
  k rollout status deployment/agentd-api --timeout=5m >/dev/null
  k rollout status deployment/agentd-loop --timeout=5m >/dev/null
}
drop_key() {
  k delete pod ant-worker --ignore-not-found --wait=false >/dev/null
  k delete secret agentd-model ant-worker --ignore-not-found >/dev/null
  roll
  echo "model key removed from the cluster"
}
trap drop_key EXIT
printf 'AGENTD_ANTHROPIC_API_KEY=%s\n' "$AGENTD_ANTHROPIC_API_KEY" | secret_from_stdin agentd-model
roll

KEY=$(cli_key keys create --org org_e2e --workspace wrkspc_e2e --name e2e)
api() {
  curl -fsS -H "x-api-key: $KEY" -H "anthropic-version: 2023-06-01" -H "anthropic-beta: managed-agents-2026-04-01" \
    -H "content-type: application/json" "$@"
}
tools='[{"type":"agent_toolset_20260401","default_config":{"permission_policy":{"type":"always_allow"}}}]'
AGENT=$(api -d "{\"name\":\"kind-e2e\",\"model\":\"claude-haiku-4-5\",\"tools\":$tools}" "$BASE_URL/v1/agents" | jq -r .id)
ENV=$(api -d '{"name":"kind-e2e","config":{"type":"self_hosted"}}' "$BASE_URL/v1/environments" | jq -r .id)
ENV_KEY=$(cli_key environment-keys create --org org_e2e --workspace wrkspc_e2e --environment "$ENV" --name e2e)
printf 'ANTHROPIC_ENVIRONMENT_ID=%s\nANTHROPIC_ENVIRONMENT_KEY=%s\n' "$ENV" "$ENV_KEY" | secret_from_stdin ant-worker
k delete pod ant-worker --ignore-not-found >/dev/null
k apply -f deploy/kind/worker.yaml >/dev/null
k wait --for=condition=Ready pod/ant-worker --timeout=2m >/dev/null

SESSION=$(api -d "{\"agent\":\"$AGENT\",\"environment_id\":\"$ENV\"}" "$BASE_URL/v1/sessions" | jq -r .id)
echo "agent $AGENT, environment $ENV, session $SESSION"
ask='Run the shell command `echo kind-e2e-ok` with your bash tool, then reply with its output only.'
api -o /dev/null -d "{\"events\":[{\"type\":\"user.message\",\"content\":[{\"type\":\"text\",\"text\":\"$ask\"}]}]}" \
  "$BASE_URL/v1/sessions/$SESSION/events"

events=
for _ in $(seq 1 90); do
  events=$(api "$BASE_URL/v1/sessions/$SESSION/events?limit=1000")
  [ "$(echo "$events" | jq -r '.data[-1] | select(.type == "session.status_idle") | .stop_reason.type')" = end_turn ] && break
  sleep 2
done
echo "$events" | jq -r '.data[].type'
echo "$events" | jq -e '[.data[] | select(.type == "agent.tool_use")] | length > 0' >/dev/null
echo "$events" | jq -e '[.data[] | select(.type == "user.tool_result") | .content[]?.text // empty
  | select(contains("kind-e2e-ok"))] | length > 0' >/dev/null
echo "$events" | jq -e '.data[-1].stop_reason.type == "end_turn"' >/dev/null
echo "reply: $(echo "$events" | jq -r '[.data[] | select(.type == "agent.message")][-1].content[0].text')"
echo "the tool ran on the worker and the turn ended"
