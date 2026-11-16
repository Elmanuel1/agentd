-- fire_claims: the fire inbox. Each scheduled fire is claimed by its target and time before it
-- runs, whichever scheduler delivered it. One replica owns a fire while claimed_at is within the
-- lease; a fire not done when the lease runs out is taken over, up to the attempt limit.
create table fire_claims (
    workspace_id text        not null,
    target       text        not null,
    scheduled_at timestamptz not null,
    claimed_at   timestamptz not null default now(),
    attempts     integer     not null default 1,
    done         boolean     not null default false,
    primary key (workspace_id, target, scheduled_at)
);

drop table deployment_claims;
