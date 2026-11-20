//! Each `sslmode` against a Postgres that serves TLS for `localhost`, signed by
//! the CA at `AGENTD_PG_TLS_CA` (`docker/postgres/start-tls.sh`), and against
//! the plain one at `DATABASE_URL`. Gated by `AGENTD_PG_TLS_CA`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use diesel::QueryableByName;
use diesel::connection::{Instrumentation, InstrumentationEvent};
use diesel::sql_types::Bool;
use diesel_async::RunQueryDsl;
use diesel_async::pooled_connection::deadpool::PoolError;
use shared::BackendError;
use shared::ports::BoxFuture;
use shared::ports::credentials::PasswordSource;
use store_postgres::{PgBackend, PgObserver, PoolSettings};

const TLS: &str = "port=5433 user=agentd password=agentd dbname=agentd";

fn ca() -> Option<String> {
    std::env::var("AGENTD_PG_TLS_CA").ok()
}

#[derive(QueryableByName)]
struct Ssl {
    #[diesel(sql_type = Bool)]
    ssl: bool,
}

/// Whether the connection `url` opens is encrypted, or why it could not open.
async fn encrypted(url: &str) -> Result<bool, String> {
    let pg = PgBackend::connect(url).await.map_err(|e| e.to_string())?;
    encrypted_on(&pg).await
}

async fn encrypted_on(pg: &PgBackend) -> Result<bool, String> {
    let mut conn = pg.diesel.get().await.map_err(|e: PoolError| e.to_string())?;
    let row: Ssl = diesel::sql_query("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
        .get_result(&mut conn)
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.ssl)
}

#[tokio::test]
async fn verify_full_with_the_ca_connects_encrypted() {
    let Some(ca) = ca() else { return };
    assert_eq!(
        encrypted(&format!("host=localhost {TLS} sslmode=verify-full sslrootcert={ca}")).await,
        Ok(true)
    );
    let url = format!("postgres://agentd:agentd@localhost:5433/agentd?sslmode=verify-full&sslrootcert={ca}");
    assert_eq!(encrypted(&url).await, Ok(true));
}

#[tokio::test]
async fn verify_full_refuses_another_host_name_and_verify_ca_accepts_it() {
    let Some(ca) = ca() else { return };
    let full = encrypted(&format!("host=127.0.0.1 {TLS} sslmode=verify-full sslrootcert={ca}")).await;
    let full = full.unwrap_err();
    assert!(full.contains("certificate not valid for name"), "{full}");
    let chain = encrypted(&format!("host=127.0.0.1 {TLS} sslmode=verify-ca sslrootcert={ca}")).await;
    assert_eq!(chain, Ok(true));
}

#[tokio::test]
async fn verify_full_refuses_a_certificate_the_roots_do_not_sign() {
    if ca().is_none() {
        return;
    }
    for root in ["sslrootcert=system", ""] {
        let got = encrypted(&format!("host=localhost {TLS} sslmode=verify-full {root}")).await;
        let got = got.unwrap_err();
        assert!(got.contains("invalid peer certificate: UnknownIssuer"), "{root:?}: {got}");
    }
}

#[tokio::test]
async fn require_and_prefer_encrypt_without_a_ca_and_disable_does_not() {
    if ca().is_none() {
        return;
    }
    for (mode, want) in [("require", true), ("prefer", true), ("disable", false)] {
        assert_eq!(encrypted(&format!("host=localhost {TLS} sslmode={mode}")).await, Ok(want), "{mode}");
    }
}

#[tokio::test]
async fn a_plain_server_takes_prefer_and_refuses_require() {
    let Ok(url) = std::env::var("DATABASE_URL") else { return };
    if ca().is_none() {
        return;
    }
    assert_eq!(encrypted(&url).await, Ok(false));
    let separator = if url.contains('?') { '&' } else { '?' };
    let required = encrypted(&format!("{url}{separator}sslmode=require")).await;
    assert!(required.unwrap_err().contains("TLS"), "require must refuse a server without TLS");
}

/// The logins it minted a password for.
#[derive(Debug, Default)]
struct Minted(Mutex<Vec<(String, u16, String)>>);

impl PasswordSource for Minted {
    fn password<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        user: &'a str,
    ) -> BoxFuture<'a, Result<String, BackendError>> {
        self.0.lock().unwrap().push((host.to_owned(), port, user.to_owned()));
        Box::pin(async { Ok("agentd".to_owned()) })
    }
}

#[derive(Clone, Copy)]
struct Quiet;

impl PgObserver for Quiet {
    fn statements(&self) -> Box<dyn Instrumentation> {
        Box::new(*self)
    }

    fn connect_failed(&self, _: &diesel::ConnectionError) {}
}

impl Instrumentation for Quiet {
    fn on_connection_event(&mut self, _: InstrumentationEvent<'_>) {}
}

#[tokio::test]
async fn each_new_connection_logs_in_with_a_minted_password() {
    let Some(ca) = ca() else { return };
    let minted = Arc::new(Minted::default());
    let url =
        format!("host=localhost port=5433 user=agentd dbname=agentd sslmode=verify-full sslrootcert={ca}");
    let settings = PoolSettings { max_size: 2, ..PoolSettings::default() };
    let pg = PgBackend::connect_with_password(&url, settings, Arc::new(Quiet), minted.clone()).await.unwrap();
    let (first, second) = (pg.diesel.get().await.unwrap(), pg.diesel.get().await.unwrap());
    drop((first, second));
    assert_eq!(encrypted_on(&pg).await, Ok(true));
    let login = ("localhost".to_owned(), 5433, "agentd".to_owned());
    assert_eq!(*minted.0.lock().unwrap(), vec![login.clone(), login]);
}

#[tokio::test]
async fn a_minted_password_is_never_sent_unencrypted() {
    for mode in ["disable", "prefer"] {
        let url = format!("host=localhost {TLS} sslmode={mode}");
        let err = PgBackend::connect_with_password(
            &url,
            PoolSettings::default(),
            Arc::new(Quiet),
            Arc::new(Minted::default()),
        )
        .await
        .map(|_| ())
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "connect: an IAM password needs sslmode=require, verify-ca or verify-full"
        );
    }
}
