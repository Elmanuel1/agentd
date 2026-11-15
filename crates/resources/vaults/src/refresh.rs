//! An `mcp_oauth` credential's refresh: the token request live sends to the
//! credential's `token_endpoint`, through the guard, so an endpoint that
//! resolves inward is never called (MC-27).

use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use shared::net::Guard;
use shared::Timestamp;

use crate::types::{OauthRefreshInput, SecretString, TokenEndpointAuthInput};

/// How long the token endpoint may take.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
/// The most of a response body an answer quotes.
const BODY_LIMIT: usize = 4096;

/// An HTTP answer, as `vault_credential_validation` quotes one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    /// Its body, cut at the limit.
    pub body: String,
    /// The body was cut.
    pub body_truncated: bool,
    /// Its `content-type`, empty when it sent none.
    pub content_type: String,
    /// Its status.
    pub status_code: u16,
}

impl HttpResponse {
    /// Quote `reply`, reading its body.
    pub async fn read(reply: reqwest::Response) -> Self {
        let status_code = reply.status().as_u16();
        let content_type = reply
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let text = reply.text().await.unwrap_or_default();
        let body_truncated = text.len() > BODY_LIMIT;
        let mut body = text;
        if body_truncated {
            let mut cut = BODY_LIMIT;
            while !body.is_char_boundary(cut) {
                cut -= 1;
            }
            body.truncate(cut);
        }
        Self { body, body_truncated, content_type, status_code }
    }
}

/// A token endpoint's answer.
#[derive(Debug)]
pub enum Refreshed {
    /// A new access token.
    Token {
        /// The token.
        access_token: SecretString,
        /// When it expires, when the endpoint said.
        expires_at: Option<Timestamp>,
        /// A rotated refresh token, when the endpoint sent one.
        refresh_token: Option<SecretString>,
    },
    /// The endpoint refused, or could not be reached (no answer).
    Failed(Option<HttpResponse>),
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: SecretString,
    expires_in: Option<i64>,
    refresh_token: Option<SecretString>,
}

/// Ask `refresh`'s token endpoint for a new access token, as live does: the
/// refresh token and client id in a form body, the client secret as the
/// stored method sends it, and no scope.
pub async fn request_token(guard: &Guard, refresh: &OauthRefreshInput) -> Refreshed {
    let Ok(url) = refresh.token_endpoint.parse::<reqwest::Url>() else { return Refreshed::Failed(None) };
    let Ok(client) = guard.client(&url, TOKEN_TIMEOUT).await else { return Refreshed::Failed(None) };
    let mut form = vec![("grant_type", "refresh_token"), ("refresh_token", refresh.refresh_token.expose())];
    if let Some(id) = &refresh.client_id {
        form.push(("client_id", id));
    }
    let mut req = client.post(url).header(reqwest::header::ACCEPT, "application/json");
    match &refresh.token_endpoint_auth {
        Some(TokenEndpointAuthInput::ClientSecretPost { client_secret }) => {
            form.push(("client_secret", client_secret.expose()));
        }
        Some(TokenEndpointAuthInput::ClientSecretBasic { client_secret }) => {
            let pair =
                format!("{}:{}", refresh.client_id.as_deref().unwrap_or_default(), client_secret.expose());
            let basic = base64::engine::general_purpose::STANDARD.encode(pair);
            req = req.header(reqwest::header::AUTHORIZATION, format!("Basic {basic}"));
        }
        Some(TokenEndpointAuthInput::None) | None => {}
    }
    let Ok(body) = serde_urlencoded::to_string(&form) else { return Refreshed::Failed(None) };
    let sent = req
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await;
    let Ok(reply) = sent else { return Refreshed::Failed(None) };
    if !reply.status().is_success() {
        return Refreshed::Failed(Some(HttpResponse::read(reply).await));
    }
    let status_code = reply.status().as_u16();
    let Ok(text) = reply.text().await else { return Refreshed::Failed(None) };
    match serde_json::from_str::<TokenBody>(&text) {
        Ok(t) => Refreshed::Token {
            access_token: t.access_token,
            expires_at: t.expires_in.map(|s| {
                Timestamp::from_datetime(Timestamp::now().as_datetime() + chrono::Duration::seconds(s))
            }),
            refresh_token: t.refresh_token,
        },
        Err(_) => Refreshed::Failed(Some(HttpResponse {
            body: text.chars().take(BODY_LIMIT).collect(),
            body_truncated: text.len() > BODY_LIMIT,
            content_type: "application/json".into(),
            status_code,
        })),
    }
}
