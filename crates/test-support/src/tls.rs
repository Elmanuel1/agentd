//! A TLS front for the tests of clients that speak TLS to a server that does
//! not (Temporal's dev server) or of a collector that is not there (OTLP): it
//! serves the test certificate `docker/tls/certs.sh` writes beside its CA.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

/// A listener on `localhost` that ends TLS and passes the bytes to `upstream`,
/// or drops them without one.
#[derive(Debug)]
pub struct TlsFront {
    /// Where it listens.
    pub addr: SocketAddr,
    handshakes: Arc<AtomicUsize>,
}

impl TlsFront {
    /// Serve `server.pem` and `server.key` from the directory `ca` is in,
    /// speaking HTTP/2 when the client asks for it.
    pub async fn start(ca: &Path, upstream: Option<SocketAddr>) -> std::io::Result<Self> {
        let dir = ca.parent().unwrap_or(Path::new("."));
        let certs = CertificateDer::pem_file_iter(dir.join("server.pem"))
            .and_then(Iterator::collect::<Result<Vec<_>, _>>)
            .map_err(std::io::Error::other)?;
        let key = PrivateKeyDer::from_pem_file(dir.join("server.key")).map_err(std::io::Error::other)?;
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .and_then(|b| b.with_no_client_auth().with_single_cert(certs, key))
            .map_err(std::io::Error::other)?;
        config.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let handshakes = Arc::new(AtomicUsize::new(0));
        let counted = handshakes.clone();
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (acceptor, counted) = (acceptor.clone(), counted.clone());
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(client).await else { return };
                    counted.fetch_add(1, Ordering::SeqCst);
                    match upstream {
                        Some(upstream) => {
                            let Ok(mut server) = TcpStream::connect(upstream).await else { return };
                            drop(tokio::io::copy_bidirectional(&mut tls, &mut server).await);
                        }
                        None => drop(tokio::io::copy(&mut tls, &mut tokio::io::sink()).await),
                    }
                });
            }
        });
        Ok(Self { addr, handshakes })
    }

    /// How many clients completed a TLS handshake.
    pub fn handshakes(&self) -> usize {
        self.handshakes.load(Ordering::SeqCst)
    }
}
