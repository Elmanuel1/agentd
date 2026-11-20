#!/bin/sh
# Delete the kind cluster (its volumes go with it) and the three images deploy/kind/up.sh built.
#   deploy/kind/down.sh
set -eu
cd "$(dirname "$0")/../.."
. deploy/kind/lib.sh
kind delete cluster --name "$CLUSTER"
docker image rm agentd:kind agentd-migrations:kind agentd-ant-worker:kind 2>/dev/null || true
