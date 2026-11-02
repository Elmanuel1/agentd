//! One request from the sandbox, after the proxy can read it: refuse it, or
//! put the session's secrets into its headers, send it on, and take them
//! out of the response again. Every substitution is logged without values.

use std::time::Duration;

use bytes::Bytes;
use http::header::{self, HeaderName};
use http::{HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;

use crate::net::public_addrs;
use crate::policy::SessionEgress;
use crate::rules::{inject, inject_body, redact, Refusal};

/// Most bytes of a request or response body the proxy handles.
const MAX_BODY: usize = 64 * 1024 * 1024;

/// Headers that describe one hop, not the request.
const HOP: [HeaderName; 7] = [
    header::CONNECTION,
    header::PROXY_AUTHORIZATION,
    header::PROXY_AUTHENTICATE,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
];

/// A plain response from the proxy itself.
pub fn reply(status: StatusCode, text: &str) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(format!("{text}\n"))));
    *r.status_mut() = status;
    r
}

fn strip_hop(headers: &mut HeaderMap) {
    for h in HOP {
        headers.remove(h);
    }
    headers.remove("proxy-connection");
    headers.remove("keep-alive");
}

/// Forward `req` for `session` to `scheme://host:port`.
pub async fn forward(
    session: &SessionEgress,
    scheme: &str,
    host: &str,
    port: u16,
    req: Request<Incoming>,
) -> Response<Full<Bytes>> {
    match forward_inner(session, scheme, host, port, req).await {
        Ok(r) => r,
        Err(r) => r,
    }
}

async fn forward_inner(
    session: &SessionEgress,
    scheme: &str,
    host: &str,
    port: u16,
    req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Response<Full<Bytes>>> {
    let (mut parts, body) = req.into_parts();
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_owned();
    let body = Limited::new(body, MAX_BODY)
        .collect()
        .await
        .map_err(|_| reply(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"))?
        .to_bytes();
    // A placeholder anywhere in the URL or body, bound for a host its
    // credential does not allow, is refused like one in a header.
    for c in &session.credentials {
        let leaks = path.contains(&c.placeholder)
            || body.windows(c.placeholder.len()).any(|w| w == c.placeholder.as_bytes());
        if leaks && !c.allows(host) {
            return Err(refused(
                session,
                host,
                &parts.method,
                &path,
                &Refusal::PlaceholderToWrongHost { name: c.name.clone() },
            ));
        }
    }
    strip_hop(&mut parts.headers);
    let mut used = inject(&mut parts.headers, &session.credentials, host)
        .map_err(|why| refused(session, host, &parts.method, &path, &why))?;
    let (body, in_body) = inject_body(&body, &session.credentials, host);
    used.extend(in_body);
    for id in &used {
        tracing::info!(target: "agentd::egress", session = %session.session, credential = %id, host, method = %parts.method, path, "secret injected");
    }
    // Plain bytes back, so a secret the server echoes can be found and masked.
    parts.headers.insert(header::ACCEPT_ENCODING, http::HeaderValue::from_static("identity"));
    let addrs = if crate::rules::any_matches(&session.internal_hosts, host) {
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
    let url = format!("{scheme}://{host}:{port}{path}");
    let method =
        reqwest::Method::from_bytes(parts.method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut headers = reqwest::header::HeaderMap::new();
    for (k, v) in &parts.headers {
        if let (Ok(k), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(k.as_ref()),
            reqwest::header::HeaderValue::from_bytes(v.as_bytes()),
        ) {
            headers.append(k, v);
        }
    }
    headers.remove(reqwest::header::HOST);
    let upstream = client.request(method, url).headers(headers).body(body).send().await;
    let upstream = upstream.map_err(|e| reply(StatusCode::BAD_GATEWAY, &format!("egress: {e}")))?;
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out_headers = HeaderMap::new();
    for (k, v) in upstream.headers() {
        if let (Ok(k), Ok(v)) =
            (HeaderName::from_bytes(k.as_ref()), http::HeaderValue::from_bytes(v.as_bytes()))
        {
            out_headers.append(k, v);
        }
    }
    let bytes =
        upstream.bytes().await.map_err(|e| reply(StatusCode::BAD_GATEWAY, &format!("egress: {e}")))?;
    let (bytes, masked) = redact(&bytes, &session.credentials);
    for id in &masked {
        tracing::warn!(target: "agentd::egress", session = %session.session, credential = %id, host, method = %parts.method, path, "secret masked in a response");
    }
    strip_hop(&mut out_headers);
    out_headers.remove(header::CONTENT_LENGTH);
    let mut response = Response::new(Full::new(Bytes::from(bytes)));
    *response.status_mut() = status;
    *response.headers_mut() = out_headers;
    Ok(response)
}

fn refused(
    session: &SessionEgress,
    host: &str,
    method: &Method,
    path: &str,
    why: &Refusal,
) -> Response<Full<Bytes>> {
    let text = match why {
        Refusal::HostNotAllowed => format!("egress: {host} is not allowed for this session"),
        Refusal::PlaceholderToWrongHost { name } => format!("egress: {name} may not be sent to {host}"),
    };
    tracing::warn!(target: "agentd::egress", session = %session.session, host, method = %method, path, reason = %text, "request refused");
    reply(StatusCode::FORBIDDEN, &text)
}
