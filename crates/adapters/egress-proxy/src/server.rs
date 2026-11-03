//! Accepting sandbox connections: the proxy user names the session and the
//! password proves it; a `CONNECT` is answered, then decrypted with a certificate from the
//! session's CA, so each request inside can be handled like a plain one.

use std::convert::Infallible;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use crate::forward::forward;
use crate::inject::Refusal;
use crate::policy::{EgressPolicy, SessionEgress};
use crate::response::reply;
use crate::secret::SecretSource;

/// The proxy user and password from `Proxy-Authorization: Basic …`.
fn credentials<B>(req: &Request<B>) -> Option<(String, String)> {
    let raw = req.headers().get(http::header::PROXY_AUTHORIZATION)?.to_str().ok()?;
    let (scheme, value) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = String::from_utf8(STANDARD.decode(value.trim()).ok()?).ok()?;
    decoded.split_once(':').map(|(u, p)| (u.to_owned(), p.to_owned()))
}

/// `host` and `port` of an authority, `default` when it names no port.
fn split_authority(authority: &str, default: u16) -> Option<(String, u16)> {
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h, p.parse().ok()?),
        _ => (authority, default),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    (!host.is_empty()).then_some((host, port))
}

/// Serve the proxy on `listener`, reading values from `source`, until the
/// task is dropped.
pub async fn serve(listener: TcpListener, policy: Arc<dyn EgressPolicy>, source: Arc<dyn SecretSource>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let (policy, source) = (policy.clone(), source.clone());
        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let (policy, source) = (policy.clone(), source.clone());
                async move { Ok::<_, Infallible>(handle(req, policy, source).await) }
            });
            let _ =
                http1::Builder::new().serve_connection(TokioIo::new(stream), service).with_upgrades().await;
        });
    }
}

async fn handle(
    req: Request<Incoming>,
    policy: Arc<dyn EgressPolicy>,
    source: Arc<dyn SecretSource>,
) -> Response<Full<Bytes>> {
    let session = match credentials(&req) {
        Some((user, password)) => policy.session(&user, &password).await,
        None => None,
    };
    let Some(session) = session else {
        let mut r = reply(StatusCode::PROXY_AUTHENTICATION_REQUIRED, "egress: proxy credentials required");
        r.headers_mut().insert(
            http::header::PROXY_AUTHENTICATE,
            http::HeaderValue::from_static("Basic realm=\"agentd\""),
        );
        return r;
    };
    if req.method() == Method::CONNECT {
        let Some((host, port)) = req.uri().authority().and_then(|a| split_authority(a.as_str(), 443)) else {
            return reply(StatusCode::BAD_REQUEST, "egress: CONNECT needs host:port");
        };
        if !session.may_reach(&host) {
            return blocked(&session, &host);
        }
        tokio::spawn(tunnel(req, session, source, host, port));
        return Response::new(Full::new(Bytes::new()));
    }
    let Some((host, port)) = req.uri().authority().and_then(|a| split_authority(a.as_str(), 80)) else {
        return reply(StatusCode::BAD_REQUEST, "egress: a proxied request needs an absolute URL");
    };
    if !session.may_reach(&host) {
        return blocked(&session, &host);
    }
    forward(&session, source.as_ref(), ("http", &host, port), req).await
}

fn blocked(session: &SessionEgress, host: &str) -> Response<Full<Bytes>> {
    tracing::warn!(target: "agentd::egress", session = %session.session, host, reason = ?Refusal::HostNotAllowed, "host blocked");
    reply(StatusCode::FORBIDDEN, &format!("egress: {host} is not allowed for this session"))
}

/// The decrypted side of a `CONNECT`.
async fn tunnel(
    req: Request<Incoming>,
    session: Arc<SessionEgress>,
    source: Arc<dyn SecretSource>,
    host: String,
    port: u16,
) {
    let config = match session.ca.server_config(&host) {
        Ok(c) => c,
        Err(e) => {
            return tracing::warn!(target: "agentd::egress", session = %session.session, host, error = %e, "no certificate")
        }
    };
    let Ok(upgraded) = hyper::upgrade::on(req).await else { return };
    let Ok(tls) = tokio_rustls::TlsAcceptor::from(config).accept(TokioIo::new(upgraded)).await else {
        return;
    };
    let service = service_fn(move |inner: Request<Incoming>| {
        let (session, source, host) = (session.clone(), source.clone(), host.clone());
        async move { Ok::<_, Infallible>(forward(&session, source.as_ref(), ("https", &host, port), inner).await) }
    });
    let _ = http1::Builder::new().serve_connection(TokioIo::new(tls), service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorities_split_into_host_and_port() {
        assert_eq!(split_authority("api.github.com:443", 443), Some(("api.github.com".into(), 443)));
        assert_eq!(split_authority("Example.com", 80), Some(("example.com".into(), 80)));
        assert_eq!(split_authority("[::1]:8443", 443), Some(("::1".into(), 8443)));
        assert_eq!(split_authority("host:notaport", 443), None);
    }

    #[test]
    fn the_session_and_password_come_from_basic_proxy_auth() {
        let auth = format!("Basic {}", STANDARD.encode("sesn_1:agentd-px-abc"));
        let req = Request::builder().header("proxy-authorization", auth).body(()).unwrap();
        assert_eq!(credentials(&req), Some(("sesn_1".into(), "agentd-px-abc".into())));
        assert_eq!(credentials(&Request::new(())), None);
    }
}
