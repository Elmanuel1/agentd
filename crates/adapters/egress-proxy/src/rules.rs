//! What the proxy does to one request and its response, as pure functions:
//! which hosts a session may reach, which placeholders become secrets in a
//! request's headers, and which secrets are masked again in a response.

use http::HeaderMap;

/// A credential a session may use: the sandbox sees `name` set to
/// `placeholder`; only the proxy ever holds `value`.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    /// The vault credential's id, for the audit log.
    pub id: String,
    /// The environment variable name the sandbox sees.
    pub name: String,
    /// The opaque per-session stand-in the sandbox sees.
    pub placeholder: String,
    /// The real value.
    pub value: String,
    /// Hosts the value may be sent to. A credential that names none is
    /// sent nowhere: "any host" would include one the agent controls.
    pub hosts: Option<Vec<String>>,
    /// Put into request headers (the only place the proxy puts a value).
    pub header: bool,
}

impl Credential {
    /// Whether the value may be sent to `host`.
    pub fn allows(&self, host: &str) -> bool {
        self.hosts.as_ref().is_some_and(|h| any_matches(h, host))
    }
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("hosts", &self.hosts)
            .finish_non_exhaustive()
    }
}

/// Whether `host` matches `rule`: a bare host matches only itself;
/// `*.example.com` matches every subdomain of `example.com`, not the apex.
pub fn host_matches(rule: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let rule = rule.trim_end_matches('.').to_ascii_lowercase();
    match rule.strip_prefix("*.") {
        Some(apex) => host.len() > apex.len() + 1 && host.ends_with(&format!(".{apex}")),
        None => rule == host,
    }
}

/// Whether any of `rules` matches `host`.
pub fn any_matches(rules: &[String], host: &str) -> bool {
    rules.iter().any(|r| host_matches(r, host))
}

/// Why a request is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The environment's `limited` networking does not allow the host.
    HostNotAllowed,
    /// A credential's placeholder was sent to a host the credential does not
    /// allow; sending it on would leak which credential the sandbox holds.
    PlaceholderToWrongHost {
        /// The credential's name.
        name: String,
    },
}

/// The credentials a request to `host` used in its headers: each placeholder
/// found is replaced by its value when the credential allows `host` and is
/// injected into headers; a placeholder bound for any other host refuses
/// the request.
pub fn inject(
    headers: &mut HeaderMap,
    credentials: &[Credential],
    host: &str,
) -> Result<Vec<String>, Refusal> {
    let mut used = Vec::new();
    for c in credentials {
        let present = headers
            .values()
            .any(|v| v.as_bytes().windows(c.placeholder.len()).any(|w| w == c.placeholder.as_bytes()));
        if !present {
            continue;
        }
        if !c.allows(host) {
            return Err(Refusal::PlaceholderToWrongHost { name: c.name.clone() });
        }
        if !c.header {
            continue;
        }
        for value in headers.values_mut() {
            let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
            if text.contains(&c.placeholder) {
                if let Ok(v) = http::HeaderValue::from_str(&text.replace(&c.placeholder, &c.value)) {
                    *value = v;
                }
            }
        }
        used.push(c.id.clone());
    }
    Ok(used)
}

/// `body` with every credential value replaced by its placeholder, and the
/// ids of the credentials found.
pub fn redact(body: &[u8], credentials: &[Credential]) -> (Vec<u8>, Vec<String>) {
    let mut out = body.to_vec();
    let mut found = Vec::new();
    for c in credentials.iter().filter(|c| !c.value.is_empty()) {
        let swapped = swap(&out, c.value.as_bytes(), c.placeholder.as_bytes());
        if swapped != out {
            found.push(c.id.clone());
            out = swapped;
        }
    }
    (out, found)
}

/// `haystack` with every `from` replaced by `to`.
fn swap(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack.get(i..i + from.len()) == Some(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.extend_from_slice(haystack.get(i..=i).unwrap_or_default());
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests;
