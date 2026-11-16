#!/bin/sh
# A one-node Valkey cluster (cluster mode enabled, every slot on one node)
# for CI and local tests, announced at 127.0.0.1 so clients on the host follow
# its slots. AOF is on, as in production.
#   start-cluster.sh <port>   (at most 55535: the bus port is 10000 higher)
set -eu
port=$1
docker run -d --name valkey-cluster -p "$port:$port" \
  valkey/valkey:9.0-alpine valkey-server --port "$port" --cluster-enabled yes \
  --cluster-announce-ip 127.0.0.1 --appendonly yes --appendfsync everysec >/dev/null
until docker exec valkey-cluster valkey-cli -p "$port" ping >/dev/null 2>&1; do sleep 1; done
docker exec valkey-cluster valkey-cli -p "$port" cluster addslotsrange 0 16383 >/dev/null
for _ in $(seq 1 30); do
  docker exec valkey-cluster valkey-cli -p "$port" cluster info | grep -q "cluster_state:ok" && exit 0
  sleep 1
done
docker logs valkey-cluster | tail -20
exit 1
