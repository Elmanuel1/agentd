# Shared by the deploy/kind scripts: the cluster, its kubectl context and agentd's namespace.
# AGENTD_KIND_CLUSTER names the cluster (default agentd); the scripts never use the current context.
CLUSTER=${AGENTD_KIND_CLUSTER:-agentd}
CTX=kind-$CLUSTER
NS=agentd
BASE_URL=http://127.0.0.1:8080
k() { kubectl --context "$CTX" -n "$NS" "$@"; }

# A key made through agentd's CLI in an api pod, printed once: `cli_key keys create --org ... --workspace ...`.
cli_key() { k exec deploy/agentd-api -- agentd "$@"; }

# A Secret from name=value pairs read on stdin, so no value appears in a process's arguments.
secret_from_stdin() { k create secret generic "$1" --from-env-file=/dev/stdin --dry-run=client -o yaml | k apply -f - >/dev/null; }
