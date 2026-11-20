#!/bin/sh
# A single-node Kafka (KRaft) for the TLS and SASL tests, its certificate for `localhost` signed by the test CA
# (docker/tls/certs.sh) in $1: SASL_SSL on localhost:19093 (PLAIN and SCRAM-SHA-512 as agentd / agentd-secret, and
# OAUTHBEARER with Kafka's unsecured tokens) and SSL on localhost:19094. Topics are created on first write.
set -eu
dir=$1
"$(dirname "$0")/../tls/certs.sh" "$dir"
mkdir -p "$dir/kafka"
cat "$dir/server.key" "$dir/server.pem" > "$dir/kafka/keystore.pem"
cat > "$dir/kafka/server.properties" <<'PROPERTIES'
node.id=1
process.roles=broker,controller
controller.quorum.voters=1@localhost:9095
controller.listener.names=CONTROLLER
listeners=PLAINTEXT://:9092,SASL_SSL://:19093,SSL://:19094,CONTROLLER://:9095
advertised.listeners=PLAINTEXT://localhost:9092,SASL_SSL://localhost:19093,SSL://localhost:19094
listener.security.protocol.map=PLAINTEXT:PLAINTEXT,SASL_SSL:SASL_SSL,SSL:SSL,CONTROLLER:PLAINTEXT
inter.broker.listener.name=PLAINTEXT
sasl.enabled.mechanisms=PLAIN,SCRAM-SHA-512,OAUTHBEARER
listener.name.sasl_ssl.plain.sasl.jaas.config=org.apache.kafka.common.security.plain.PlainLoginModule required user_agentd="agentd-secret";
listener.name.sasl_ssl.scram-sha-512.sasl.jaas.config=org.apache.kafka.common.security.scram.ScramLoginModule required;
listener.name.sasl_ssl.oauthbearer.sasl.jaas.config=org.apache.kafka.common.security.oauthbearer.OAuthBearerLoginModule required;
ssl.keystore.type=PEM
ssl.keystore.location=/mnt/shared/config/keystore.pem
offsets.topic.replication.factor=1
transaction.state.log.replication.factor=1
transaction.state.log.min.isr=1
group.initial.rebalance.delay.ms=0
log.dirs=/tmp/kraft-combined-logs
PROPERTIES
docker run -d --name kafka-secure -p 127.0.0.1:19093:19093 -p 127.0.0.1:19094:19094 -v "$dir/kafka:/mnt/shared/config" \
  apache/kafka:3.9.1 >/dev/null
for _ in $(seq 1 60); do
  if docker exec kafka-secure /opt/kafka/bin/kafka-configs.sh --bootstrap-server localhost:9092 --alter \
    --add-config 'SCRAM-SHA-512=[password=agentd-secret]' --entity-type users --entity-name agentd >/dev/null 2>&1; then
    exit 0
  fi
  sleep 2
done
docker logs kafka-secure | tail -20
exit 1
