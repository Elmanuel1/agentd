# Resource history: dependencies

Each fact the design rests on, with its evidence. `VERIFIED` rows were read in the code, in Postgres's or
Debezium's documentation, or measured against Postgres 16 and Debezium Server 3.3.1; `ASSUMED` rows name what
reopens them.

## agentd today

| # | Fact | Status | Evidence |
|---|---|---|---|
| A1 | Every write goes through `DieselPgPool::write()`, which sets `agentd.actor` and `agentd.request_id` on the connection, session-level, before the write. No write runs on a read checkout. | VERIFIED | `store-postgres/src/checkout.rs` `write`; inventory of the 72 write and 64 read checkouts |
| A2 | The actors are a key id (`apikey_…`, `adminkey_…`, an environment key id), `runtime`, a work token's session (`sesn_…`), a schedule's key, `operator` (the CLI) and `system` (the loop, the webhook sender, the scheduler tick, retention), each with its own request id. | VERIFIED | `shared/src/audit.rs` `Writer`; `api/src/edge/auth.rs` |
| A3 | None of the 18 tracked tables has `actor` or `request_id`, and no store code writes them; the coordination tables that have them fill them by column default, on insert only. | VERIFIED | migrations V1 to V15 |
| A4 | Memory content lives in blobs. A version's row holds its `content_sha256`, `content_size_bytes` and `path` in `doc`; redaction nulls them and deletes the blob. A head's row holds the same three for its current version. | VERIFIED | `memory/src/service/versions.rs` `redact_version`; `store-postgres/src/entities/memory/versions.rs` `version_doc` |
| A5 | The change feed hands each handler `table`, `op`, `before` and `after`, and drops logical messages. Topic and stream names were built in each adapter. | VERIFIED | `shared/src/ports/changes.rs` before this change |

## Postgres 16

| # | Fact | Status | Evidence |
|---|---|---|---|
| B1 | `pg_logical_emit_message(true, prefix, content)` writes a transactional message that is decoded only if its transaction commits; it is executable by every role and works at any `wal_level`. | VERIFIED | Postgres docs, "Replication Management Functions"; `proacl` is the default on Postgres 16; run at `wal_level=replica` |
| B2 | Without `REPLICA IDENTITY FULL`, an update that leaves a TOASTed column unchanged decodes without that column's value; with it, the old row is logged whole. | VERIFIED | Postgres docs, `ALTER TABLE ... REPLICA IDENTITY`; [Morling, TOAST and Debezium](https://www.morling.dev/blog/backfilling-postgres-toast-columns-debezium-change-events/) |
| B3 | `ADD COLUMN` with a constant default and `REPLICA IDENTITY` change only the catalog; creating a trigger takes a brief `SHARE ROW EXCLUSIVE` lock. | VERIFIED | Postgres docs, `ALTER TABLE` notes |
| B4 | A replication slot keeps all the WAL it has not confirmed, of every table; past `max_slot_wal_keep_size` Postgres invalidates it (`wal_status = 'lost'`) and the changes in it are gone. | VERIFIED | Postgres docs, `pg_replication_slots`; Debezium docs, PostgreSQL connector |

## Debezium Server 3.3.1

| # | Fact | Status | Evidence |
|---|---|---|---|
| C1 | A change event's `source` carries `lsn`, `txId` and `ts_ms` (the commit time). | VERIFIED | Debezium docs, PostgreSQL connector, "Change event values" |
| C2 | A logical message arrives as `op: "m"` on the topic `<topic.prefix>.message`, its `message.prefix` and its `message.content` base64-encoded in JSON; `message.prefix.include.list` keeps only the prefixes it names. | VERIFIED | Debezium docs, PostgreSQL connector, "Logical decoding message events" |
| C3 | `jsonb` columns arrive as JSON text and `bytea` columns as base64. | VERIFIED | Debezium docs, data type mappings; `schedules/outbox.rs` reads `change` as text |
| C4 | `publication.autocreate.mode=filtered` creates the publication for `table.include.list`, and adds the tables a longer list names to an existing publication at start. | VERIFIED | Debezium docs, `publication.autocreate.mode`; measured: the publication went from 2 to 20 tables on restart |
| C5 | With `REPLICA IDENTITY FULL`, an update that leaves a TOASTed column unchanged arrives with that column whole in `after`, not as `__debezium_unavailable_value`. | VERIFIED | measured: a 12.8 KB out-of-line `doc`, `crates/api/tests/history.rs` |
| C6 | A transactional message that rolls back never reaches the feed; a committed one arrives on `agentd.message` with `source.lsn`, `txId` and `ts_ms`. | VERIFIED | measured against Debezium Server 3.3.1 on Kafka |

## Load

| # | Fact | Status | Evidence |
|---|---|---|---|
| N1 | The sizing peak is 10,000 concurrent sessions, about 5,000 event appends a second and 500 session updates a second. | VERIFIED | `.specs/runtime-parity/dependencies.md`, the SRE section |
| N2 | A session row is about 4 KB, so an update with `REPLICA IDENTITY FULL` writes about 8 KB of WAL. | ASSUMED | the minimal fixture is 0.8 KB; reopened by a load replay that measures `agentd.feed.wal_written` |
| N3 | At the peak the WAL is about 20 MB a second (72 GB an hour); history in Postgres grows about 170 GB a day, mostly sessions. | ASSUMED | N1 and N2; reopened by the same replay |
