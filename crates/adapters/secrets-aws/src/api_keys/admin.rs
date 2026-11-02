//! What `agentd keys` runs: create, list and revoke. A listing reads only
//! tags, never a value.

use aws_sdk_secretsmanager::types::{Filter, FilterNameStringType, SecretListEntry, Tag};
use shared::charset::is_id_charset;
use shared::ports::keys::KeyError;
use shared::ports::secrets::{ExposeSecret, SecretString};
use shared::Id;

use super::{key_error, material, ApiKeyRecord, ApiKeyResource, AwsApiKeys, Entry, SHOWN};
use crate::failure::failure;

/// A name a tag value can carry: 1 to 128 of letters, digits, spaces and
/// `_ . : / = + - @`.
fn name_ok(name: &str) -> bool {
    (1..=128).contains(&name.chars().count())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || " _.:/=+-@".contains(c))
}

fn tag(key: &str, value: &str) -> Tag {
    Tag::builder().key(key).value(value).build()
}

impl AwsApiKeys {
    /// A new key for `org` and `workspace`, both in the id charset. The key is
    /// in the return value only: it is not stored and cannot be read again.
    pub async fn create(
        &self,
        org: &str,
        workspace: &str,
        name: &str,
    ) -> Result<(ApiKeyRecord, SecretString), KeyError> {
        if !is_id_charset(org) || !is_id_charset(workspace) {
            return Err(KeyError("the org and workspace ids must be in the id charset".into()));
        }
        if !name_ok(name) {
            return Err(KeyError(
                "the name must be 1 to 128 letters, digits, spaces or any of _ . : / = + - @".into(),
            ));
        }
        let key = material::generate();
        let digest = material::digest(&self.pepper, key.expose_secret())
            .ok_or_else(|| KeyError("the pepper cannot key a digest".into()))?;
        let id = Id::<ApiKeyResource>::generate().to_string();
        let hint = material::hint(key.expose_secret());
        let digest = hex::encode(digest);
        let entry = Entry {
            digest: digest.clone(),
            api_key_id: id.clone(),
            org: org.into(),
            workspace: workspace.into(),
        };
        let value = serde_json::to_string(&entry).map_err(|e| KeyError(e.to_string()))?;
        self.store
            .client
            .create_secret()
            .name(format!("{}{digest}", self.dir()))
            .kms_key_id(&self.store.key_arn)
            .secret_string(value)
            .client_request_token(uuid::Uuid::new_v4().to_string())
            .set_tags(Some(vec![
                tag("managed-by", "agentd"),
                tag("api-key-id", &id),
                tag("org", org),
                tag("workspace", workspace),
                tag("key-name", name),
                tag("key-hint", &hint),
            ]))
            .send()
            .await
            .map_err(|e| key_error(failure("secretsmanager:CreateSecret", SHOWN, &e)))?;
        let record = ApiKeyRecord {
            id,
            org: org.into(),
            workspace: workspace.into(),
            name: name.into(),
            partial_key_hint: hint,
            created_at: chrono::Utc::now().to_rfc3339(),
            revoked: false,
        };
        Ok((record, key))
    }

    /// Every key, revoked ones included, from the tags alone: no value is read.
    pub async fn list(&self) -> Result<Vec<ApiKeyRecord>, KeyError> {
        self.entries(true, None).await.map(|entries| entries.iter().filter_map(record).collect())
    }

    /// Revoke `id`: its secret is deleted with the recovery window. `false`
    /// when no active key has it.
    pub async fn revoke(&self, id: &str) -> Result<bool, KeyError> {
        let found = self.entries(false, Some(id)).await?;
        let Some(name) = found.iter().filter_map(|e| e.name.as_deref()).next() else {
            return Ok(false);
        };
        self.store.delete_named(name, SHOWN).await.map_err(key_error)?;
        Ok(true)
    }

    /// The key secrets, optionally those scheduled for deletion, optionally
    /// only the one tagged `id`. Secrets Manager filters by prefix and case
    /// insensitively, and an emulator may list deleted secrets regardless, so
    /// the names, tags and deletion are checked exactly here.
    async fn entries(&self, revoked: bool, id: Option<&str>) -> Result<Vec<SecretListEntry>, KeyError> {
        let dir = self.dir();
        let mut filters = vec![Filter::builder().key(FilterNameStringType::Name).values(&dir).build()];
        if let Some(id) = id {
            filters.push(Filter::builder().key(FilterNameStringType::TagKey).values("api-key-id").build());
            filters.push(Filter::builder().key(FilterNameStringType::TagValue).values(id).build());
        }
        let mut pages = self
            .store
            .client
            .list_secrets()
            .set_filters(Some(filters))
            .include_planned_deletion(revoked)
            .into_paginator()
            .send();
        let mut out = Vec::new();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|e| key_error(failure("secretsmanager:ListSecrets", &dir, &e)))?;
            out.extend(page.secret_list.unwrap_or_default().into_iter().filter(|e| {
                e.name.as_deref().is_some_and(|n| n.starts_with(&dir))
                    && (revoked || e.deleted_date.is_none())
                    && id.is_none_or(|id| tag_value(e, "api-key-id") == Some(id))
            }));
        }
        Ok(out)
    }
}

fn tag_value<'a>(entry: &'a SecretListEntry, key: &str) -> Option<&'a str> {
    entry.tags.as_deref()?.iter().find(|t| t.key.as_deref() == Some(key))?.value.as_deref()
}

fn record(entry: &SecretListEntry) -> Option<ApiKeyRecord> {
    let created_at =
        entry.created_date.and_then(|d| d.fmt(aws_smithy_types::date_time::Format::DateTime).ok());
    Some(ApiKeyRecord {
        id: tag_value(entry, "api-key-id")?.into(),
        org: tag_value(entry, "org")?.into(),
        workspace: tag_value(entry, "workspace")?.into(),
        name: tag_value(entry, "key-name").unwrap_or_default().into(),
        partial_key_hint: tag_value(entry, "key-hint").unwrap_or_default().into(),
        created_at: created_at.unwrap_or_default(),
        revoked: entry.deleted_date.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_what_a_tag_value_can_carry() {
        for ok in ["ci", "sdk conformance", "team/a@x.io", &"n".repeat(128)] {
            assert!(name_ok(ok), "{ok}");
        }
        for bad in ["", "a\tb", "a\nb", "é", "a&b", "<x>", &"n".repeat(129)] {
            assert!(!name_ok(bad), "{bad:?}");
        }
    }
}
