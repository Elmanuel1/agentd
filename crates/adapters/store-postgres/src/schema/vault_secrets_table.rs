//! `vault_secrets` (V20).

diesel::table! {
    vault_secrets (workspace_id, vault_id, credential_id) {
        workspace_id -> Text,
        vault_id -> Text,
        credential_id -> Text,
        environment_variable -> Bool,
        key_id -> Text,
        sealed_key -> Bytea,
        sealed_value -> Bytea,
    }
}
