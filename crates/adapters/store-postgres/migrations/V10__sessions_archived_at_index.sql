-- Event retention walks archived sessions in (archived_at, session_id) order.
-- CONCURRENTLY, so writes to sessions continue while it builds; Flyway runs this file
-- outside a transaction. A failed build leaves an INVALID index and a failed history
-- row: drop the index, run `flyway repair`, then migrate again.
create index concurrently sessions_archived_at
    on sessions ((doc->>'archived_at'), session_id)
    where doc->>'archived_at' is not null;
