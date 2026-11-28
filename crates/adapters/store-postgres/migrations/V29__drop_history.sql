-- agentd keeps no history store: the change feed's events are the record. The writer columns,
-- the `history_stamp` and `history_deleted` triggers and `replica identity full` stay, so each
-- event carries its writer and the whole row.
set local lock_timeout = '5s';

drop schema history cascade;
