use std::path::PathBuf;

use super::{ConnInfo, RootCert, SslMode, split};

fn info(rest: &str, mode: SslMode, root: Option<RootCert>) -> ConnInfo {
    ConnInfo { rest: rest.to_owned(), mode, root }
}

#[test]
fn a_url_without_tls_parameters_is_prefer_and_unchanged() {
    let url = "postgres://agentd:pw@localhost:5432/agentd";
    assert_eq!(split(url).unwrap(), info(url, SslMode::Prefer, None));
}

#[test]
fn a_url_loses_its_tls_parameters_and_keeps_the_rest() {
    let url = "postgresql://u:p@db:5432/agentd?application_name=a&sslmode=verify-full&sslrootcert=%2Fetc%2Fca.pem&connect_timeout=5";
    let want = info(
        "postgresql://u:p@db:5432/agentd?application_name=a&connect_timeout=5",
        SslMode::VerifyFull,
        Some(RootCert::File(PathBuf::from("/etc/ca.pem"))),
    );
    assert_eq!(split(url).unwrap(), want);
}

#[test]
fn a_url_with_only_tls_parameters_drops_the_query() {
    let got = split("postgres://db/agentd?sslmode=disable").unwrap();
    assert_eq!(got, info("postgres://db/agentd", SslMode::Disable, None));
}

#[test]
fn keywords_lose_their_tls_parameters_and_keep_quoted_values() {
    let conninfo =
        r"host=db port=5432 password='p w\'x' sslmode = verify-ca sslrootcert='/a b/ca.pem' dbname=agentd";
    let want = info(
        r"host=db port=5432 password='p w\'x' dbname=agentd",
        SslMode::VerifyCa,
        Some(RootCert::File(PathBuf::from("/a b/ca.pem"))),
    );
    assert_eq!(split(conninfo).unwrap(), want);
}

#[test]
fn system_names_the_operating_system_roots() {
    let got = split("host=db sslmode=verify-full sslrootcert=system").unwrap();
    assert_eq!(got, info("host=db", SslMode::VerifyFull, Some(RootCert::System)));
}

#[test]
fn require_with_a_root_certificate_checks_the_chain() {
    let got = split("host=db sslmode=require sslrootcert=/ca.pem").unwrap();
    assert_eq!(got.mode, SslMode::VerifyCa);
    assert_eq!(split("host=db sslmode=require").unwrap().mode, SslMode::Require);
}

#[test]
fn every_libpq_mode_but_allow_is_read() {
    for (value, mode) in [
        ("disable", SslMode::Disable),
        ("prefer", SslMode::Prefer),
        ("require", SslMode::Require),
        ("verify-ca", SslMode::VerifyCa),
        ("verify-full", SslMode::VerifyFull),
    ] {
        assert_eq!(split(&format!("host=db sslmode={value}")).unwrap().mode, mode);
    }
    let err = split("host=db sslmode=allow").unwrap_err();
    assert_eq!(
        err,
        "sslmode `allow` is not supported: use disable, prefer, require, verify-ca or verify-full"
    );
}

#[test]
fn a_malformed_keyword_string_is_refused() {
    assert_eq!(split("host db").unwrap_err(), "the connection string has no `=` after `host`");
    assert_eq!(
        split("password='open").unwrap_err(),
        "the connection string has an unterminated quoted value"
    );
}
