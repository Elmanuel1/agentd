//! Putting a session's secrets into one request: each credential whose
//! placeholder the request carries is read from the store now; a request to
//! a host the credential does not allow is refused, as is one whose value
//! cannot be read (fail closed). Values go into headers only, in locked
//! buffers the caller drops once the request is sent.

use http::{HeaderMap, HeaderValue};
use shared::ports::secrets::SecretError;
use zeroize::Zeroizing;

use crate::locked::Locked;
use crate::mask::Fingerprint;
use crate::rules::{replace, Binding};
use crate::secret::SecretSource;

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
    /// The credential is no longer in the store.
    CredentialGone {
        /// The credential's name.
        name: String,
    },
    /// The store could not be read, or the value could not be held safely.
    Unavailable {
        /// The credential's name.
        name: String,
    },
}

/// The secrets put into one request.
#[derive(Debug, Default)]
pub struct Injection {
    /// The credentials used, by id.
    pub used: Vec<String>,
    /// The buffers the injected header values share; drop them once the
    /// request is sent, and they are zeroed.
    pub held: Vec<Locked>,
    /// What is kept to mask the values in the response.
    pub prints: Vec<Fingerprint>,
}

/// Put into `headers` the credentials of `bindings` that a request to `host`
/// with `path` and `body` carries. On a refusal `headers` is cleared, so the
/// values already put in are zeroed.
pub async fn inject(
    headers: &mut HeaderMap,
    path: &str,
    body: &[u8],
    bindings: &[Binding],
    source: &dyn SecretSource,
    host: &str,
) -> Result<Injection, Refusal> {
    let mut injection = Injection::default();
    match fill(headers, (path, body), bindings, source, host, &mut injection).await {
        Ok(()) => Ok(injection),
        Err(why) => {
            headers.clear();
            drop(injection);
            Err(why)
        }
    }
}

async fn fill(
    headers: &mut HeaderMap,
    (path, body): (&str, &[u8]),
    bindings: &[Binding],
    source: &dyn SecretSource,
    host: &str,
    injection: &mut Injection,
) -> Result<(), Refusal> {
    for b in bindings {
        let in_headers = headers.values().any(|v| b.found_in(v.as_bytes()));
        if !in_headers && !b.found_in(path.as_bytes()) && !b.found_in(body) {
            continue;
        }
        let name = b.name.clone();
        let current = match source.current(&b.key).await {
            Ok(c) => c,
            Err(SecretError::NotFound(_)) => return Err(Refusal::CredentialGone { name }),
            Err(e) => {
                tracing::warn!(target: "agentd::egress", credential = %b.id, error = %e, "credential store unreadable");
                return Err(Refusal::Unavailable { name });
            }
        };
        if !current.allows(host) {
            return Err(Refusal::PlaceholderToWrongHost { name });
        }
        if !in_headers || !current.header {
            continue;
        }
        for value in headers.values_mut() {
            let Some(swapped) = replace(value.as_bytes(), b.placeholder.as_bytes(), current.value.expose())
            else {
                continue;
            };
            let swapped = Zeroizing::new(swapped);
            let locked = Locked::new(&swapped).map_err(|e| {
                tracing::warn!(target: "agentd::egress", credential = %b.id, error = %e, "secret not held");
                Refusal::Unavailable { name: name.clone() }
            })?;
            *value = HeaderValue::from_maybe_shared(locked.bytes())
                .map_err(|_| Refusal::Unavailable { name: name.clone() })?;
            injection.held.push(locked);
        }
        injection.prints.push(Fingerprint::of(&b.id, &b.placeholder, current.value.expose()));
        injection.used.push(b.id.clone());
    }
    Ok(())
}
