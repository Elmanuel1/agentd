#!/bin/sh
# A Valkey that serves only TLS on localhost:6390, its certificate for `localhost` signed by the test CA
# (docker/tls/certs.sh) in $1, for the TLS tests.
set -eu
dir=$1
"$(dirname "$0")/../tls/certs.sh" "$dir"
docker run -d --name valkey-tls -p 6390:6390 -v "$dir:/certs:ro" valkey/valkey:8.0-alpine valkey-server \
  --port 0 --tls-port 6390 --tls-cert-file /certs/server.pem --tls-key-file /certs/server.key \
  --tls-ca-cert-file /certs/ca.pem --tls-auth-clients no >/dev/null
for _ in $(seq 1 30); do
  docker exec valkey-tls valkey-cli --tls --cacert /certs/ca.pem -h localhost -p 6390 ping >/dev/null 2>&1 && exit 0
  sleep 1
done
docker logs valkey-tls | tail -20
exit 1
