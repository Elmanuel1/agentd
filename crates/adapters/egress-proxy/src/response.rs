//! The upstream's response on its way back to the sandbox: read up to a
//! limit, refused when an injected secret could hide in an encoding the
//! proxy cannot read, and masked. And the hop-by-hop headers either way.

use bytes::Bytes;
use http::header::{self, HeaderName};
use http::{HeaderMap, Response, StatusCode};
use http_body_util::Full;

use crate::mask::{mask, Fingerprint};

/// Most bytes of a request or response body the proxy handles.
pub const MAX_BODY: usize = 64 * 1024 * 1024;

/// Headers that describe one hop, not the message.
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

/// Remove the hop-by-hop headers, and first every header `Connection` names
/// (RFC 9110 7.6.1).
pub fn strip_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::from_bytes(t.trim().as_bytes()).ok())
        .collect();
    for h in named.into_iter().chain(HOP) {
        headers.remove(h);
    }
    headers.remove("proxy-connection");
    headers.remove("keep-alive");
}

/// `upstream` as the sandbox gets it, and the credentials masked in it.
pub async fn back(
    mut upstream: reqwest::Response,
    prints: &[Fingerprint],
) -> Result<(Response<Full<Bytes>>, Vec<String>), Response<Full<Bytes>>> {
    let encoding = upstream.headers().get(header::CONTENT_ENCODING).and_then(|v| v.to_str().ok());
    if !prints.is_empty() && encoding.is_some_and(|e| !e.eq_ignore_ascii_case("identity")) {
        // A secret the server echoed could not be found in encoded bytes.
        return Err(reply(StatusCode::BAD_GATEWAY, "egress: the response is encoded, so it cannot be checked for the credential sent; it was not passed on"));
    }
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = HeaderMap::new();
    let mut found = Vec::new();
    for (k, v) in upstream.headers() {
        // A secret echoed in a header is masked as one in the body is.
        let (value, masked) = mask(v.as_bytes(), prints);
        found.extend(masked);
        if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_ref()), http::HeaderValue::from_bytes(&value)) {
            headers.append(k, v);
        }
    }
    let mut body = Vec::new();
    while let Some(chunk) =
        upstream.chunk().await.map_err(|e| reply(StatusCode::BAD_GATEWAY, &format!("egress: {e}")))?
    {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(reply(StatusCode::BAD_GATEWAY, "egress: the response is larger than 64 MiB"));
        }
        body.extend_from_slice(&chunk);
    }
    let (body, masked) = mask(&body, prints);
    found.extend(masked);
    strip_hop(&mut headers);
    headers.remove(header::CONTENT_LENGTH);
    let mut response = Response::new(Full::new(Bytes::from(body)));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok((response, found))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_connection_names_go_with_the_hop_headers() {
        let mut h = HeaderMap::new();
        h.insert(header::CONNECTION, "x-secret-hop, keep-alive".parse().unwrap());
        h.insert("x-secret-hop", "1".parse().unwrap());
        h.insert("x-kept", "1".parse().unwrap());
        h.insert(header::TE, "trailers".parse().unwrap());
        strip_hop(&mut h);
        assert_eq!(h.keys().map(HeaderName::as_str).collect::<Vec<_>>(), ["x-kept"]);
    }
}
