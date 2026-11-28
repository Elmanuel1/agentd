-- When each sealed secret was last written, so the cleanup sweep leaves one written ahead of a
-- credential row that has not committed yet.
set local lock_timeout = '5s';

alter table vault_secrets add column written_at timestamptz not null default now();
