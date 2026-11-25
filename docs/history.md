# Change records

agentd keeps no history of its own. Every change to a tracked table reaches the change feed (`configuration.md`,
"The change feed") with who made it and the whole row, and the feed's consumers keep whatever record they need.
The API does not change: there is no history route.

## What the feed carries

The tracked tables are `agents`, `deployments`, `deployment_runs`, `files`, `vaults`, `credentials`, `skills`,
`skill_versions`, `memory_stores`, `memories`, `memory_versions`, `sessions`, `dreams`, `dream_runs`, `api_keys`,
`workspaces`, `environment_keys`, `admin_keys`, `environments` and `organizations`.

Each tracked table's `history_stamp()` trigger sets `actor` and `request_id` on every insert and update from the
writing connection (`agentd.actor`, `agentd.request_id`, which every write sets). `actor` is the key behind the
request (`apikey_…`, `adminkey_…`, an environment key id), `runtime`, a work token's session (`sesn_…`), `operator`
for the CLI, or `system` for the agent loop, the scheduler and retention. Its `history_deleted()` trigger writes
each delete to the WAL as a logical message, `agentd-history`, in the deleting transaction, carrying the table,
the row as it was, and the deleter as its actor. Each tracked table has `REPLICA IDENTITY FULL`, so an update that
leaves a large `doc` unchanged still reaches the feed whole, and a delete carries its old row.

Debezium Server reads these changes into the topics `agentd.public.<table>` and the delete messages into
`agentd.message` (`docker/debezium/application.properties`).

Not on the feed: the coordination tables (`work_items`, `loop_steps`, `mcp_connections`, `schedules`,
`fire_claims`), the session event log, the Postgres secret store's sealed credentials (`vault_secrets`), and the
blob and secret content of a file, skill, memory or credential.

## The slot

The change feed's replication slot (`AGENTD_CHANGES_SLOT`, default `agentd_changes`) holds every change not yet
on the feed. It keeps all the WAL Debezium has not confirmed, of every table, while Debezium, Kafka or Valkey is
down. Past `max_slot_wal_keep_size` Postgres invalidates it (`wal_status = 'lost'`), and the changes it held never
reach the feed.

agentd reads the slot every 10 s on every replica (take the maximum across replicas):

| Signal | Alarm when | What it means, and what to do |
|---|---|---|
| `agentd.feed.slot.safe_wal` | below 25% of `max_slot_wal_keep_size` (page) | the feed has stalled long enough to near the limit: bring Debezium and its sink back before the slot is lost |
| `agentd.feed.slot.lost` | 1 (page) | the slot is invalidated (`wal_status = 'lost'`) and the feed has a gap: follow "After a lost slot". It also reads 1 while the slot does not exist yet, before Debezium Server first starts |
| `agentd.feed.slot.retained_wal` | growing for 15 minutes (ticket) | Debezium is not confirming: check its health (`GET /q/health`) and logs |

While a replica cannot read the slot, its slot values are absent, never the last reading: alarm when no replica
reports `agentd.feed.slot.lost` for 5 minutes (page). `agentd.feed.wal_written` counts the WAL Postgres writes, for
sizing.

### WAL sizing

`max_slot_wal_keep_size` = the peak WAL rate (from `agentd.feed.wal_written`) × the longest feed outage to ride
out. At the sizing peak (10,000 sessions) the WAL is about 20 MB a second: event appends about 15 MB, session
updates with their old rows about 4 MB, the rest about 1 MB. That is 72 GB an hour; riding out 4 hours takes
about 290 GB of disk. Compose keeps 1 GB, which covers about 50 s at the peak and about 85 minutes at 1% of it.

### The feed's own durability

On Kafka, the topics' retention must exceed the longest time every consumer may be down, or records expire before
it reads them. On Valkey Streams, AOF with `appendfsync everysec` loses up to 1 s of entries on a Valkey crash;
`appendfsync always` closes that.

## After a lost slot

1. Stop Debezium Server.
2. Drop the slot, if it still exists, and Debezium's offsets, with `<slot>` the configured `AGENTD_CHANGES_SLOT`:
   `select pg_drop_replication_slot(slot_name) from pg_replication_slots where slot_name = '<slot>';` and
   `delete from debezium_offsets;`.
3. Start Debezium Server. It makes a new slot and streams changes from then on.

Changes committed between the slot's last confirmed position and the new slot are not on the feed; consumers that
keep a record rebuild it from the live tables.

## A new tracked table

A new tracked table gets the two triggers, the `actor` and `request_id` columns, `REPLICA IDENTITY FULL` and an
entry in Debezium's `table.include.list`, in the same change. A test fails when the include list misses a table
with a `_history_stamp` trigger (`store-postgres/tests/debezium_include.rs`).
