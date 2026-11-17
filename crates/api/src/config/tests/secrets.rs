//! `AGENTD_SECRETS` and its stores' settings.

use shared::hash::SecretKeys;

use super::*;

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn ring() -> String {
    format!("k1:{KEY}")
}

#[test]
fn unset_with_no_kms_key_is_postgres_under_the_master_keys() {
    for unset in ["", "  "] {
        let c = config(&[("AGENTD_SECRETS_KMS_KEY_ID", unset), ("AGENTD_SECRET_KEYS", &ring())]).unwrap();
        assert_eq!(c.secrets, SecretsConfig::Postgres(SecretKeys::parse(&ring()).unwrap()));
    }
}

#[test]
fn a_kms_key_selects_aws_sm_unless_the_selector_says_otherwise() {
    for unset in [None, Some(""), Some(" ")] {
        let vars: Vec<_> = unset.map(|v| ("AGENTD_SECRETS", v)).into_iter().collect();
        assert!(matches!(config(&vars).unwrap().secrets, SecretsConfig::AwsSm(_)), "{unset:?}");
    }
    let c = config(&[("AGENTD_SECRETS", "postgres"), ("AGENTD_SECRET_KEYS", &ring())]).unwrap();
    assert!(matches!(c.secrets, SecretsConfig::Postgres(_)), "a set selector wins over the KMS key");
    let c = config(&[("AGENTD_SECRETS", "aws")]).unwrap();
    assert!(matches!(c.secrets, SecretsConfig::AwsSm(_)), "`aws` also selects Secrets Manager");
    for unknown in ["vault", "ssm", "sm"] {
        assert_eq!(config(&[("AGENTD_SECRETS", unknown)]).unwrap_err().var, "AGENTD_SECRETS", "{unknown}");
    }
}

#[test]
fn postgres_needs_32_byte_keys_and_names_a_bad_entry_by_its_position() {
    let short = &KEY[..32];
    let cases = [
        (String::new(), "required"),
        ("  ".to_owned(), "required"),
        (format!("k1:{short}"), "entry 1 is not"),
        (format!("{KEY}:k1"), "entry 1 is not"),
        (format!("k1:{KEY},k2{KEY}"), "entry 2 is not"),
    ];
    for (raw, why) in cases {
        let err = config(&[("AGENTD_SECRETS", "postgres"), ("AGENTD_SECRET_KEYS", &raw)]).unwrap_err();
        assert_eq!(err.var, "AGENTD_SECRET_KEYS", "{raw}");
        assert!(err.reason.contains(why) && !err.to_string().contains(&KEY[..8]), "{err}");
    }
}

#[test]
fn aws_ssm_takes_the_aws_settings_but_no_recovery_window() {
    let c = config(&[("AGENTD_SECRETS", "aws_ssm"), ("AGENTD_SECRETS_PREFIX", "agentd/prod")]).unwrap();
    let SecretsConfig::AwsSsm(ssm) = c.secrets else { panic!("{:?}", c.secrets) };
    assert_eq!((ssm.prefix.as_str(), ssm.kms_key_id.as_str()), ("agentd/prod", "alias/agentd"));
    let err = config(&[("AGENTD_SECRETS", "aws_ssm"), ("AGENTD_SECRETS_RECOVERY_DAYS", "7")]).unwrap_err();
    assert_eq!(err.var, "AGENTD_SECRETS_RECOVERY_DAYS");
}
