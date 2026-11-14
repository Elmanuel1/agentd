#!/bin/sh
# CI's change feed. GitHub's service containers take no command, so logical
# WAL goes on with ALTER SYSTEM and a restart, and AOF with CONFIG SET; then
# Debezium Server runs on the host's network, after the schema exists, writing
# to the Valkey on localhost:<port>, a cluster when <cluster> is true.
#   start-ci.sh <postgres container> <valkey container> <valkey port> <cluster>
set -eu
pg=$1 valkey=$2 port=$3 cluster=$4
docker exec "$pg" psql -U agentd -d agentd -qc "ALTER SYSTEM SET wal_level = logical"
docker restart "$pg" >/dev/null
until docker exec "$pg" pg_isready -U agentd -d agentd >/dev/null 2>&1; do sleep 1; done
docker exec "$valkey" valkey-cli -p "$port" CONFIG SET appendonly yes >/dev/null
docker exec "$valkey" valkey-cli -p "$port" CONFIG SET appendfsync everysec >/dev/null
docker run -d --name debezium --network host \
  -v "$PWD/docker/debezium:/debezium/config:ro" \
  -e DEBEZIUM_SOURCE_DATABASE_HOSTNAME=localhost \
  -e DEBEZIUM_SOURCE_DATABASE_PASSWORD=agentd \
  -e DEBEZIUM_SOURCE_OFFSET_STORAGE_JDBC_CONNECTION_URL=jdbc:postgresql://localhost:5432/agentd \
  -e DEBEZIUM_SOURCE_OFFSET_STORAGE_JDBC_CONNECTION_PASSWORD=agentd \
  -e DEBEZIUM_SINK_REDIS_ADDRESS="localhost:$port" \
  -e DEBEZIUM_SINK_REDIS_CLUSTER_ENABLED="$cluster" \
  quay.io/debezium/server:3.3.1.Final >/dev/null
# Ready once its slot streams.
slot="select count(*) from pg_replication_slots where slot_name = 'agentd_changes' and active"
for _ in $(seq 1 120); do
  [ "$(docker exec "$pg" psql -U agentd -d agentd -tAc "$slot")" = 1 ] && exit 0
  sleep 1
done
docker logs debezium 2>&1 | tail -50
exit 1
