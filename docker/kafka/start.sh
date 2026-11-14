#!/bin/sh
# A single-node Kafka (KRaft) on localhost:9092 for CI and local tests, with
# topics created on first write.
set -eu
docker run -d --name kafka -p 9092:9092 apache/kafka:3.9.1 >/dev/null
for _ in $(seq 1 60); do
  docker exec kafka /opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:9092 --list >/dev/null 2>&1 && exit 0
  sleep 2
done
docker logs kafka | tail -20
exit 1
