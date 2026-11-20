//! The database one pool connects to: its connection string read once, its
//! TLS, and the password source when each login takes a fresh password.

use std::sync::Arc;

use diesel::ConnectionError;
use diesel_async::AsyncPgConnection;
use shared::ports::credentials::PasswordSource;
use tokio_postgres::config::{Host, SslMode as PgSslMode};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::PgError;
use crate::conninfo::{self, SslMode};

pub(crate) struct Target {
    config: tokio_postgres::Config,
    tls: Option<MakeRustlsConnect>,
    password: Option<Arc<dyn PasswordSource>>,
}

impl Target {
    /// A minted password is a credential, so it is only sent over a
    /// connection that must be encrypted.
    pub(crate) fn parse(url: &str, password: Option<Arc<dyn PasswordSource>>) -> Result<Self, PgError> {
        let info = conninfo::split(url).map_err(PgError::Connect)?;
        if password.is_some() && matches!(info.mode, SslMode::Disable | SslMode::Prefer) {
            return Err(PgError::Connect(
                "an IAM password needs sslmode=require, verify-ca or verify-full".into(),
            ));
        }
        let mut config: tokio_postgres::Config =
            info.rest.parse().map_err(|e: tokio_postgres::Error| PgError::Connect(e.to_string()))?;
        config.ssl_mode(match info.mode {
            SslMode::Disable => PgSslMode::Disable,
            SslMode::Prefer => PgSslMode::Prefer,
            SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => PgSslMode::Require,
        });
        let tls = crate::tls::connector(info.mode, info.root.as_ref()).map_err(PgError::Connect)?;
        Ok(Self { config, tls, password })
    }

    pub(crate) async fn establish(&self) -> Result<AsyncPgConnection, ConnectionError> {
        let mut config = self.config.clone();
        if let Some(source) = &self.password {
            let (host, port, user) = self.login()?;
            let password = source.password(host, port, user).await.map_err(bad)?;
            config.password(password);
        }
        match &self.tls {
            Some(tls) => {
                let (client, connection) = config.connect(tls.clone()).await.map_err(failed)?;
                AsyncPgConnection::try_from_client_and_connection(client, connection).await
            }
            None => {
                let (client, connection) = config.connect(tokio_postgres::NoTls).await.map_err(failed)?;
                AsyncPgConnection::try_from_client_and_connection(client, connection).await
            }
        }
    }

    /// The host, port and user a minted password is for.
    fn login(&self) -> Result<(&str, u16, &str), ConnectionError> {
        let host = match self.config.get_hosts().first() {
            Some(Host::Tcp(host)) => host.as_str(),
            _ => return Err(bad("an IAM password needs a TCP host")),
        };
        let port = self.config.get_ports().first().copied().unwrap_or(5432);
        let user = self.config.get_user().ok_or_else(|| bad("an IAM password needs a user"))?;
        Ok((host, port, user))
    }
}

fn bad(e: impl std::fmt::Display) -> ConnectionError {
    ConnectionError::BadConnection(e.to_string())
}

/// The error with its causes: a failed handshake says why only in its source.
fn failed(e: tokio_postgres::Error) -> ConnectionError {
    let mut message = e.to_string();
    let mut cause = std::error::Error::source(&e);
    while let Some(c) = cause {
        message.push_str(&format!(": {c}"));
        cause = c.source();
    }
    ConnectionError::BadConnection(message)
}
