//! Standard Webhooks signing: `v1,` and the base64 HMAC-SHA256, keyed with
//! the decoded part of a `whsec_` secret, over `{id}.{timestamp}.{body}`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{KeyInit, Mac};
use sha2::Sha256;

/// An endpoint's signing key, decoded from its `whsec_` secret.
#[derive(Clone, PartialEq, Eq)]
pub struct SigningSecret(Vec<u8>);

impl std::fmt::Debug for SigningSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SigningSecret(***)")
    }
}

impl SigningSecret {
    /// `whsec_<base64>`, or the base64 alone.
    pub fn parse(secret: &str) -> Option<Self> {
        let key = secret.strip_prefix("whsec_").unwrap_or(secret);
        STANDARD.decode(key).ok().filter(|k| !k.is_empty()).map(Self)
    }
}

/// The `webhook-signature` of `body` sent as `id` at `timestamp` (seconds).
pub fn sign(secret: &SigningSecret, id: &str, timestamp: i64, body: &str) -> String {
    let Ok(mut mac) = <hmac::Hmac<Sha256> as KeyInit>::new_from_slice(&secret.0) else {
        return String::new();
    };
    mac.update(format!("{id}.{timestamp}.{body}").as_bytes());
    format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_is_standard_webhooks_v1() {
        // The Standard Webhooks reference vector.
        let secret = SigningSecret::parse("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw").unwrap();
        let body = r#"{"test": 2432232314}"#;
        assert_eq!(
            sign(&secret, "msg_p5jXN8AQM9LWM0D4loKWxJek", 1_614_265_330, body),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
    }
}
