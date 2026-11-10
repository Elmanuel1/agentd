//! `environment_keys` (V9).

diesel::table! {
    environment_keys (environment_key_id) {
        environment_key_id -> Text,
        org_id -> Text,
        workspace_id -> Text,
        environment_id -> Text,
        name -> Text,
        key_digest -> Bytea,
        partial_key_hint -> Text,
        created_at -> Timestamptz,
        revoked_at -> Nullable<Timestamptz>,
    }
}
