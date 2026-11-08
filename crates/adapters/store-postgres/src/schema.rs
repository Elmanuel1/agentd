//! Diesel table definitions — the typed schema, mirroring the Flyway
//! migrations (V1 + V2). Flyway remains the migration authority; this is the
//! `diesel print-schema` equivalent kept in lockstep (same split as Flyway →
//! jOOQ in the Java stack). Column types map: `text`→Text, `bigint`/`bigserial`
//! →Int8, `timestamptz`→Timestamptz, `jsonb`→Jsonb, `boolean`→Bool.

mod api_keys_table;
mod workspaces_table;
pub use api_keys_table::api_keys;
pub use workspaces_table::workspaces;

diesel::table! {
    agents (workspace_id, agent_id, version) {
        workspace_id -> Text,
        agent_id -> Text,
        version -> Int8,
        name -> Text,
        archived_at -> Nullable<Timestamptz>,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    deployments (workspace_id, deployment_id) {
        workspace_id -> Text,
        deployment_id -> Text,
        name -> Text,
        status -> Text,
        archived_at -> Nullable<Timestamptz>,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    deployment_runs (workspace_id, deployment_id, run_id) {
        workspace_id -> Text,
        deployment_id -> Text,
        run_id -> Text,
        created_at -> Timestamptz,
        has_error -> Bool,
        doc -> Jsonb,
    }
}

diesel::table! {
    deployment_claims (workspace_id, deployment_id, scheduled_at) {
        workspace_id -> Text,
        deployment_id -> Text,
        scheduled_at -> Text,
        claimed_at -> Timestamptz,
    }
}

diesel::table! {
    files (workspace_id, file_id) {
        workspace_id -> Text,
        file_id -> Text,
        filename -> Text,
        scope_id -> Nullable<Text>,
        size_bytes -> Int8,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    environments (org_id, environment_id) {
        org_id -> Text,
        environment_id -> Text,
        name -> Text,
        state -> Text,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    vaults (workspace_id, vault_id) {
        workspace_id -> Text,
        vault_id -> Text,
        display_name -> Text,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    credentials (workspace_id, vault_id, credential_id) {
        workspace_id -> Text,
        vault_id -> Text,
        credential_id -> Text,
        display_name -> Text,
        has_secret -> Bool,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    skills (workspace_id, skill_id) {
        workspace_id -> Text,
        skill_id -> Text,
        display_name -> Text,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    skill_versions (workspace_id, skill_id, version) {
        workspace_id -> Text,
        skill_id -> Text,
        version -> Int8,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    memory_stores (workspace_id, store_id) {
        workspace_id -> Text,
        store_id -> Text,
        name -> Text,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    memories (workspace_id, store_id, memory_id) {
        workspace_id -> Text,
        store_id -> Text,
        memory_id -> Text,
        path -> Text,
        head_version_id -> Text,
        deleted -> Bool,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    memory_versions (workspace_id, store_id, version_id) {
        workspace_id -> Text,
        store_id -> Text,
        memory_id -> Text,
        version_id -> Text,
        seq -> Int8,
        has_blob -> Bool,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    sessions (workspace_id, session_id) {
        workspace_id -> Text,
        session_id -> Text,
        status -> Text,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    session_events (workspace_id, session_id, event_id) {
        workspace_id -> Text,
        session_id -> Text,
        event_id -> Text,
        seq -> Int8,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    dreams (workspace_id, dream_id) {
        workspace_id -> Text,
        dream_id -> Text,
        name -> Text,
        status -> Text,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}

diesel::table! {
    dream_runs (workspace_id, dream_id, run_id) {
        workspace_id -> Text,
        dream_id -> Text,
        run_id -> Text,
        seq -> Int8,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}
