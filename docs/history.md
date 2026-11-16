# Resource history

agentd keeps every change to its resources, with who made it and when, in the Postgres schema `history`. The live
tables and the API do not change: there is no history route, and reading, searching and rolling back history are
the SQL statements below. The design is `.specs/resource-history/` (ADR 0001).

## What is kept

The tracked tables are `agents`, `deployments`, `deployment_runs`, `files`, `vaults`, `credentials`, `skills`,
`skill_versions`, `memory_stores`, `memories`, `memory_versions`, `sessions`, `dreams`, `dream_runs`, `api_keys`,
`workspaces`, `environment_keys` and `admin_keys`. Each has a history table, `history.<table>`, with the same
columns and types and four of its own:

| Column | Meaning |
|---|---|
| `lsn` | Where the change sits in the WAL; with the live key, the history table's key |
| `tx_id` | The change's transaction |
| `op` | `insert`, `update`, `delete`, or `baseline` for a row recorded by the baseline below |
| `committed_at` | When the change's transaction committed |

`actor` and `request_id` are the writer of that change: the key behind the request (`apikey_…`, `adminkey_…`, an
environment key id), `runtime`, a work token's session (`sesn_…`), `operator` for the CLI, or `system` for the
agent loop, the scheduler and retention, each with its request id. A `delete` row holds the row as it was, with the
deleter as its actor.

Not kept: environments, which keep their own revisions (`environment_revisions`); the coordination tables
(`work_items`, `loop_steps`, `mcp_connections`, `schedules`, `fire_claims`, `schedule_outbox`,
`webhook_deliveries`); the session event log, which is the session's history; and the blob and secret content of a
deleted file, skill, memory or credential, which the delete removes.

## How it is recorded

Each tracked table's `history_stamp()` trigger sets `actor` and `request_id` on every insert and update from the
writing connection (`agentd.actor`, `agentd.request_id`, which every write sets). Its `history_deleted()` trigger
writes each delete to the WAL as a logical message, `agentd-history`, in the deleting transaction. Each tracked
table has `REPLICA IDENTITY FULL`, so an update that leaves a large `doc` unchanged still reaches the feed whole.

Debezium Server reads these changes and messages into the change feed (`configuration.md`, "The change feed"), and
every replica's history consumer records each one into its history table, once however often it is delivered. A
change that cannot be recorded (a table with no history table, a row that does not decode) is logged at `ERROR`
(`history: a change cannot be recorded; passed over`) and is a gap a person must look at.

While history catches up after a feed outage, its recorders share the replica's Postgres pool, one connection per
tracked table at a time; `db.client.connection.pending_requests` shows when requests wait for them.

Redacting a memory version removes its `content_sha256`, `content_size_bytes` and `path` from that version's
history rows and from the rows of every head of its memory that held it, a head deleted later included, in
whichever order the feed delivers them.

## Read a row's history

```sql
select lsn, op, actor, request_id, committed_at, display_name, doc
  from history.vaults
 where workspace_id = '<workspace_id>' and vault_id = '<vault_id>'
 order by lsn;
```

## Search

`history.changes` unions every history table's `table_name`, `row_key` (the live key as JSON), `lsn`, `op`,
`actor`, `request_id` and `committed_at`. Each history table has an index on `(actor, committed_at)` and one on
`request_id`, so both searches read each table by index.

What an actor changed in a time range:

```sql
select table_name, row_key, op, request_id, committed_at
  from history.changes
 where actor = '<actor>' and committed_at >= '<from>' and committed_at < '<to>'
 order by committed_at;
```

What one request changed:

```sql
select table_name, row_key, op, actor, committed_at
  from history.changes
 where request_id = '<request_id>'
 order by committed_at;
```

## Roll back a row

Run as `operator`, so the stamp trigger records the rollback as a change by `operator`, which the feed then
records like any other. Name the table's own columns, less `actor` and `request_id`, which the trigger sets. Pick
the `lsn` of the history row to go back to: the row as it was after that change.

A live row, back to how it was at that change:

```sql
set agentd.actor = 'operator';
set agentd.request_id = '<ticket>';
update vaults v
   set display_name = h.display_name, created_at = h.created_at, doc = h.doc
  from history.vaults h
 where h.workspace_id = '<workspace_id>' and h.vault_id = '<vault_id>' and h.lsn = '<lsn>'
   and v.workspace_id = h.workspace_id and v.vault_id = h.vault_id;
```

A deleted row, back as it was at that change (the `delete` row's `lsn` brings it back as it was deleted):

```sql
set agentd.actor = 'operator';
set agentd.request_id = '<ticket>';
insert into vaults (workspace_id, vault_id, display_name, created_at, doc)
select workspace_id, vault_id, display_name, created_at, doc
  from history.vaults
 where workspace_id = '<workspace_id>' and vault_id = '<vault_id>' and lsn = '<lsn>';
```

A rollback restores the row, not what lived outside it: a deleted file's, skill's or memory's blob and a deleted
credential's secret are gone.

## The slot

The change feed's replication slot (`AGENTD_CHANGES_SLOT`, default `agentd_changes`) is the one way to lose
history. It keeps all the WAL Debezium has not confirmed, of every table, while Debezium, Kafka or Valkey is down.
Past `max_slot_wal_keep_size` Postgres invalidates it (`wal_status = 'lost'`), and the changes it held never reach
history.

agentd reads the slot every 10 s on every replica (take the maximum across replicas):

| Signal | Alarm when | What it means, and what to do |
|---|---|---|
| `agentd.feed.slot.safe_wal` | below 25% of `max_slot_wal_keep_size` (page) | the feed has stalled long enough to near the limit: bring Debezium and its sink back before the slot is lost |
| `agentd.feed.slot.lost` | 1 (page) | the slot is lost or missing and history has a gap: follow "After a lost slot" |
| `agentd.feed.slot.retained_wal` | growing for 15 minutes (ticket) | Debezium is not confirming: check its health (`GET /q/health`) and logs |
| `agentd.history.apply_delay` p99 | over 5 minutes (ticket) | history lags its commits: check the feed backlog and the `history:` log lines |

`agentd.feed.wal_written` counts the WAL Postgres writes, for sizing.

### WAL sizing

`max_slot_wal_keep_size` = the peak WAL rate (from `agentd.feed.wal_written`) × the longest feed outage to ride
out. At the sizing peak (10,000 sessions) the WAL is about 20 MB a second: event appends about 15 MB, session
updates with their old rows about 4 MB, the rest about 1 MB. That is 72 GB an hour; riding out 4 hours takes
about 290 GB of disk. Compose keeps 1 GB, which covers about 50 s at the peak and about 85 minutes at 1% of it.

History itself grows about 170 GB a day at the peak in Postgres, most of it sessions; a deployment near the peak
moves history to a columnar store behind the same `HistoryLog` port.

### The feed's own durability

On Kafka, the topics' retention must exceed the longest time every replica may be down, or records expire before
history reads them. On Valkey Streams, AOF with `appendfsync everysec` loses up to 1 s of entries on a Valkey crash;
`appendfsync always` closes that.

## After a lost slot

1. Stop Debezium Server.
2. Drop the slot and Debezium's offsets: `select pg_drop_replication_slot('agentd_changes');` and
   `delete from debezium_offsets;`.
3. Start Debezium Server. It makes a new slot and streams changes from then on.
4. Record every live row as a `baseline`, one statement per tracked table:

```sql
do $$
declare
    t text;
    cols text;
begin
    for t in select c.relname from pg_class c join pg_namespace n on n.oid = c.relnamespace
              where n.nspname = 'history' and c.relkind = 'r'
    loop
        select string_agg(quote_ident(a.attname), ', ' order by a.attnum) into cols
          from pg_attribute a
         where a.attrelid = format('public.%I', t)::regclass and a.attnum > 0 and not a.attisdropped;
        execute format(
            'insert into history.%I (%s, lsn, op, committed_at) select %s, pg_current_wal_lsn(), %L, now() '
            'from public.%I on conflict do nothing',
            t, cols, cols, 'baseline', t);
    end loop;
end $$;
```

The changes between the loss and the baseline are gone; the `agentd.feed.slot.lost` gauge's samples bound the
window. The same baseline starts history on a database that already has rows.

## A new column or tracked table

A migration that adds a column to a tracked table adds it to `history.<table>` in the same migration. A new
tracked table gets the two triggers, `REPLICA IDENTITY FULL`, a history table with its two indexes, a branch of
`history.changes` and an entry in Debezium's `table.include.list`, in the same change. Tests fail when a tracked
table's columns differ from its history table's, when a history table is missing from `history.changes`, or when
the include list misses one.
