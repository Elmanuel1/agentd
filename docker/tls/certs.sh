#!/bin/sh
# A test CA (ca.pem) and a certificate it signs for `localhost` (server.pem, server.key), in $1, for the TLS tests'
# servers. Made once: a second call keeps them.
set -eu
dir=$1
mkdir -p "$dir"
[ -f "$dir/server.pem" ] && exit 0
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=agentd-test-ca \
  -keyout "$dir/ca.key" -out "$dir/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj /CN=localhost -keyout "$dir/server.key" -out "$dir/server.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost\n' > "$dir/server.ext"
openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.pem" -CAkey "$dir/ca.key" -CAcreateserial -days 2 \
  -extfile "$dir/server.ext" -out "$dir/server.pem" 2>/dev/null
chmod 644 "$dir/server.key"
