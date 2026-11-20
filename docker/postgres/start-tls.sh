#!/bin/sh
# A Postgres on localhost:5433 that serves TLS with a certificate for `localhost`, signed by a CA made here, for the
# TLS tests. $1: the directory the CA certificate (ca.pem) is written to.
set -eu
dir=$1
mkdir -p "$dir"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=agentd-test-ca \
  -keyout "$dir/ca.key" -out "$dir/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj /CN=localhost -keyout "$dir/server.key" -out "$dir/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost\n' > "$dir/server.ext"
openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.pem" -CAkey "$dir/ca.key" -CAcreateserial -days 2 \
  -extfile "$dir/server.ext" -out "$dir/server.pem" 2>/dev/null
docker run -d --name postgres-tls -p 5433:5432 -v "$dir:/certs:ro" \
  -e POSTGRES_USER=agentd -e POSTGRES_PASSWORD=agentd -e POSTGRES_DB=agentd \
  --entrypoint sh postgres:16-alpine -c '
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
