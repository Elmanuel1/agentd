#!/bin/sh
# CI's change feed. GitHub's service containers take no command, so logical
# WAL goes on with ALTER SYSTEM and a restart, and AOF with CONFIG SET; then
# Debezium Server runs on the services' network, after the schema exists.
#   start-ci.sh <postgres container> <valkey container> <network>
set -eu
pg=$1 valkey=$2 network=$3
docker exec "$pg" psql -U agentd -d agentd -qc "ALTER SYSTEM SET wal_level = logical"
docker restart "$pg" >/dev/null
until docker exec "$pg" pg_isready -U agentd -d agentd >/dev/null 2>&1; do sleep 1; done
docker exec "$valkey" valkey-cli CONFIG SET appendonly yes >/dev/null
docker exec "$valkey" valkey-cli CONFIG SET appendfsync everysec >/dev/null
docker run -d --name debezium --network "$network" \
  -v "$PWD/docker/debezium:/debezium/config:ro" \
  -e DEBEZIUM_SOURCE_DATABASE_PASSWORD=agentd \
  -e DEBEZIUM_SOURCE_OFFSET_STORAGE_JDBC_CONNECTION_PASSWORD=agentd \
  quay.io/debezium/server:3.3.1.Final >/dev/null
# Ready once its slot streams.
slot="select count(*) from pg_replication_slots where slot_name = 'agentd_changes' and active"
for _ in $(seq 1 120); do
  [ "$(docker exec "$pg" psql -U agentd -d agentd -tAc "$slot")" = 1 ] && exit 0
  sleep 1
done
docker logs debezium 2>&1 | tail -50
exit 1
