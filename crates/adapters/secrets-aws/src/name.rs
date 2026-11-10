//! Secret names and tags.
//!
//! A name is `<prefix>/<workspace>/<vault>/<credential>`. The key parts are in
//! the id charset ([`SecretKey`] checks them when it is made) and the prefix
//! is checked at connect, so no input can name a secret outside the prefix or
//! in another tenant's path. Neither allows `-`, so no name ends like the
//! `-xxxxxx` suffix Secrets Manager adds to ARNs, and a partial ARN never
//! matches two secrets.

use aws_sdk_secretsmanager::types::Tag;
use shared::ports::secrets::{SecretError, SecretKey};

/// The longest prefix: the name stays well inside Secrets Manager's 512.
const PREFIX_MAX: usize = 128;

/// `prefix` is 1 to 128 characters of `/`-separated segments, each of ASCII
/// letters, digits and `_`.
pub(crate) fn check_prefix(prefix: &str) -> Result<(), SecretError> {
    let segments_ok =
        prefix.split('/').all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
    if prefix.len() <= PREFIX_MAX && segments_ok {
        return Ok(());
    }
    Err(SecretError::Backend(format!(
        "the secret name prefix `{prefix}` must be 1 to {PREFIX_MAX} characters of `/`-separated \
         segments of ASCII letters, digits and `_`"
    )))
}

pub(crate) fn secret_name(prefix: &str, key: &SecretKey) -> String {
    format!("{prefix}/{key}")
}

/// The tags a created secret carries: what the IAM policy's conditions and an
/// operator's inventory read.
pub(crate) fn tags(key: &SecretKey) -> Vec<Tag> {
    let kind = key.is_environment_variable().then_some(CREDENTIAL_TYPE);
    [("workspace", key.workspace()), ("vault", key.vault()), ("managed-by", "agentd")]
        .into_iter()
        .chain(kind)
        .map(|(k, v)| Tag::builder().key(k).value(v).build())
        .collect()
}

/// The tag an `environment_variable` credential's secret carries: the egress
/// swap's IAM policy reaches secrets with it and no others.
pub(crate) const CREDENTIAL_TYPE: (&str, &str) = ("credential_type", "environment_variable");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_the_prefix_then_the_key() {
        let key = SecretKey::new("wrkspc_01a", "vlt_01b", "cred_01c").unwrap();
        assert_eq!(secret_name("agentd/prod", &key), "agentd/prod/wrkspc_01a/vlt_01b/cred_01c");
    }

    #[test]
    fn only_an_environment_variable_secret_carries_the_credential_type_tag() {
        let key = SecretKey::new("wrkspc_01a", "vlt_01b", "cred_01c").unwrap();
        let names = |k: &SecretKey| {
            tags(k)
                .iter()
                .map(|t| format!("{}={}", t.key().unwrap_or(""), t.value().unwrap_or("")))
                .collect::<Vec<_>>()
        };
        assert!(!names(&key).iter().any(|t| t.starts_with("credential_type")));
        let tagged = names(&key.environment_variable());
        assert!(tagged.contains(&"credential_type=environment_variable".to_owned()), "{tagged:?}");
    }

    #[test]
    fn a_prefix_takes_only_plain_segments() {
        for good in ["agentd", "agentd/prod", "a_b/C9", &"a".repeat(PREFIX_MAX)] {
            assert!(check_prefix(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "/agentd",
            "agentd/",
            "a//b",
            "a/../b",
            "a/./b",
            "agentd*",
            "agent?",
            "a-b",
            "a b",
            "agentd\n",
            "é",
            &"a".repeat(PREFIX_MAX + 1),
        ] {
            assert!(matches!(check_prefix(bad), Err(SecretError::Backend(_))), "{bad:?}");
        }
    }

    #[test]
    fn tags_name_the_workspace_vault_and_owner() {
        let key = SecretKey::new("wrkspc_01a", "vlt_01b", "cred_01c").unwrap();
        let tags: Vec<_> = tags(&key).into_iter().map(|t| (t.key, t.value)).collect();
        let want = [("workspace", "wrkspc_01a"), ("vault", "vlt_01b"), ("managed-by", "agentd")];
        assert_eq!(tags, want.map(|(k, v)| (Some(k.to_owned()), Some(v.to_owned()))));
    }
}
