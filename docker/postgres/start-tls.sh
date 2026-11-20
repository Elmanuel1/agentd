#!/bin/sh
# A Postgres on localhost:5433 that serves TLS with a certificate for `localhost`, signed by the test CA
# (docker/tls/certs.sh), for the TLS tests. $1: the directory the CA certificate (ca.pem) is written to.
set -eu
dir=$1
"$(dirname "$0")/../tls/certs.sh" "$dir"
docker run -d --name postgres-tls -p 127.0.0.1:5433:5432 -v "$dir:/certs:ro" \
  -e POSTGRES_USER=agentd -e POSTGRES_PASSWORD=agentd -e POSTGRES_DB=agentd \
  --entrypoint sh postgres:17-alpine -c '
    install -o postgres -m 600 /certs/server.key /var/lib/postgresql/server.key
    install -o postgres -m 644 /certs/server.pem /var/lib/postgresql/server.pem
    exec docker-entrypoint.sh postgres -c ssl=on \
      -c ssl_cert_file=/var/lib/postgresql/server.pem -c ssl_key_file=/var/lib/postgresql/server.key' >/dev/null
for _ in $(seq 1 60); do
  docker exec postgres-tls pg_isready -h localhost -U agentd >/dev/null 2>&1 && exit 0
  sleep 1
done
docker logs postgres-tls | tail -20
exit 1
