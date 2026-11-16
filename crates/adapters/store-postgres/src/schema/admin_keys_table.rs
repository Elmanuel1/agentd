//! `admin_keys` (V15).

diesel::table! {
    admin_keys (admin_key_id) {
        admin_key_id -> Text,
        org_id -> Text,
        name -> Text,
        key_digest -> Bytea,
        partial_key_hint -> Text,
        created_at -> Timestamptz,
        revoked_at -> Nullable<Timestamptz>,
    }
}
