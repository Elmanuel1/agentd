#!/bin/sh
# CI's change feed. GitHub's service containers take no command, so logical
# WAL goes on with ALTER SYSTEM and a restart; then Debezium Server runs on
# the host's network, after the schema exists, writing to the sink named.
#   start-ci.sh <postgres container> kafka <brokers>
#   start-ci.sh <postgres container> redis <address> <cluster: true|false>
set -eu
pg=$1 sink=$2
case "$sink" in
  kafka) set -- -e DEBEZIUM_SINK_TYPE=kafka -e DEBEZIUM_SINK_KAFKA_PRODUCER_BOOTSTRAP_SERVERS="$3" ;;
  redis) set -- -e DEBEZIUM_SINK_TYPE=redis -e DEBEZIUM_SINK_REDIS_ADDRESS="$3" -e DEBEZIUM_SINK_REDIS_CLUSTER_ENABLED="$4" ;;
  *) echo "unknown sink $sink" >&2; exit 2 ;;
esac
docker exec "$pg" psql -U agentd -d agentd -qc "ALTER SYSTEM SET wal_level = logical"
docker restart "$pg" >/dev/null
until docker exec "$pg" pg_isready -U agentd -d agentd >/dev/null 2>&1; do sleep 1; done
docker run -d --name debezium --network host \
  -v "$PWD/docker/debezium:/debezium/config:ro" \
  -e DEBEZIUM_SOURCE_DATABASE_HOSTNAME=localhost \
  -e DEBEZIUM_SOURCE_DATABASE_PASSWORD=agentd \
  -e DEBEZIUM_SOURCE_OFFSET_STORAGE_JDBC_CONNECTION_URL=jdbc:postgresql://localhost:5432/agentd \
  -e DEBEZIUM_SOURCE_OFFSET_STORAGE_JDBC_CONNECTION_PASSWORD=agentd \
  "$@" quay.io/debezium/server:3.3.1.Final >/dev/null
# Ready once its slot streams.
slot="select count(*) from pg_replication_slots where slot_name = 'agentd_changes' and active"
for _ in $(seq 1 120); do
  [ "$(docker exec "$pg" psql -U agentd -d agentd -tAc "$slot")" = 1 ] && exit 0
  sleep 1
done
docker logs debezium 2>&1 | tail -50
exit 1
