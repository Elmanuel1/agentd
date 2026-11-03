//! One request from the sandbox, after the proxy can read it: refuse it, or
//! put the session's secrets into its headers, send it on, zero them, and
//! mask them in the response. Every substitution is logged without values.

use std::time::Duration;

use bytes::Bytes;
use http::header;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;

use crate::inject::{inject, Refusal};
use crate::net::public_addrs;
use crate::policy::SessionEgress;
use crate::response::{back, reply, strip_hop, MAX_BODY};
use crate::secret::SecretSource;

/// Forward `req` for `session` to `scheme://host:port`.
pub async fn forward(
    session: &SessionEgress,
    source: &dyn SecretSource,
    (scheme, host, port): (&str, &str, u16),
    req: Request<Incoming>,
) -> Response<Full<Bytes>> {
    match forward_inner(session, source, (scheme, host, port), req).await {
        Ok(r) => r,
        Err(r) => r,
    }
}

async fn forward_inner(
    session: &SessionEgress,
    source: &dyn SecretSource,
    (scheme, host, port): (&str, &str, u16),
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Response<Full<Bytes>>> {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_owned();
    let body = Limited::new(body, MAX_BODY)
        .collect()
        .await
        .map_err(|_| reply(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"))?
        .to_bytes();
    strip_hop(&mut parts.headers);
    // Plain bytes back, so a secret the server echoes can be found and masked.
    parts.headers.insert(header::ACCEPT_ENCODING, http::HeaderValue::from_static("identity"));
    let internal = crate::rules::any_matches(&session.internal_hosts, host);
    let addrs = if internal {
        tokio::net::lookup_host((host, port))
            .await
            .map(Iterator::collect::<Vec<_>>)
            .map_err(|e| e.to_string())
    } else {
        public_addrs(host, port).await
    };
    let addrs = addrs.map_err(|e| reply(StatusCode::FORBIDDEN, &format!("egress: {e}")))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addrs)
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| reply(StatusCode::BAD_GATEWAY, &format!("egress: {e}")))?;
    // Last before sending, so no early return leaves a value in a buffer.
    // A credential crosses the network in cleartext only to an internal host.
    let cleartext = scheme == "http" && !internal;
    let injection = inject(&mut parts.headers, (&path, &body), &session.bindings, source, host, cleartext)
        .await
        .map_err(|why| refused(session, host, &parts.method, &path, &why))?;
    for id in &injection.used {
        tracing::info!(target: "agentd::egress", session = %session.session, credential = %id, host, method = %parts.method, path, "secret injected");
    }
    let url = format!("{scheme}://{host}:{port}{path}");
    let method = parts.method.clone();
    // reqwest shares http's types: the injected values go out as they are,
    // with no copy the proxy could not zero.
    let mut headers = std::mem::take(&mut parts.headers);
    headers.remove(header::HOST);
    let upstream = client.request(method, url).headers(headers).body(body).send().await;
    let (held, prints) = (injection.held, injection.prints);
    // The request is sent and the client holds no copy of it: zero the values.
    drop(held);
    let upstream = upstream.map_err(|e| reply(StatusCode::BAD_GATEWAY, &format!("egress: {e}")))?;
    let (response, masked) = back(upstream, &prints).await?;
    for id in &masked {
        tracing::warn!(target: "agentd::egress", session = %session.session, credential = %id, host, method = %parts.method, path, "secret masked in a response");
    }
    Ok(response)
}

fn refused(
    session: &SessionEgress,
    host: &str,
    method: &Method,
    path: &str,
    why: &Refusal,
) -> Response<Full<Bytes>> {
    let (status, text) = match why {
        Refusal::HostNotAllowed => {
            (StatusCode::FORBIDDEN, format!("egress: {host} is not allowed for this session"))
        }
        Refusal::PlaceholderToWrongHost { name } => {
            (StatusCode::FORBIDDEN, format!("egress: {name} may not be sent to {host}"))
        }
        Refusal::CredentialGone { name } => {
            (StatusCode::FORBIDDEN, format!("egress: {name} no longer exists"))
        }
        Refusal::Cleartext { name } => {
            (StatusCode::FORBIDDEN, format!("egress: {name} is sent to {host} over HTTPS only"))
        }
        Refusal::Unavailable { name } => {
            (StatusCode::BAD_GATEWAY, format!("egress: {name} cannot be read now; the request was not sent"))
        }
    };
    tracing::warn!(target: "agentd::egress", session = %session.session, host, method = %method, path, reason = %text, "request refused");
    reply(status, &text)
}
