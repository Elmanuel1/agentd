//! How each backing service logs in.

use super::*;

#[test]
fn postgres_logs_in_with_the_url_password_unless_iam_is_set() {
    let StoreConfig::Postgres { auth, .. } = config(&[]).unwrap().store;
    assert_eq!(auth, PgAuth::Password);
    let StoreConfig::Postgres { auth, .. } = config(&[("AGENTD_PG_AUTH", "aws_iam")]).unwrap().store;
    assert_eq!(auth, PgAuth::AwsIam);
    let err = config(&[("AGENTD_PG_AUTH", "gcp_iam")]).unwrap_err();
    assert_eq!(
        (err.var, err.reason.as_str()),
        ("AGENTD_PG_AUTH", "unknown method `gcp_iam`: use password or aws_iam")
    );
}
