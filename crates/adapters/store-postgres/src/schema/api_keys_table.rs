//! `api_keys` (V4).

diesel::table! {
    api_keys (api_key_id) {
        api_key_id -> Text,
        org_id -> Text,
        workspace_id -> Text,
        name -> Text,
        key_digest -> Bytea,
        partial_key_hint -> Text,
        created_at -> Timestamptz,
        revoked_at -> Nullable<Timestamptz>,
    }
}
