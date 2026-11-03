//! `workspaces` (V6).

diesel::table! {
    workspaces (org_id, workspace_id) {
        org_id -> Text,
        workspace_id -> Text,
        archived_at -> Nullable<Timestamptz>,
        created_at -> Timestamptz,
        doc -> Jsonb,
    }
}
