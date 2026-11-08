//! HTTP transport for the RomM API: headers, token-host binding, TLS mode,
//! error translation and the retry policy.
//!
//! Every request carries the plugin `User-Agent` (Cloudflare's Bot Fight Mode
//! rejects anonymous clients). The bearer token is attached only when its
//! minting origin matches the configured `romm_url`; a mismatch refuses the
//! request with [`RommError::TokenHostMismatch`] before any byte leaves.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::{Map, Value};

use crate::domain::errors::RommError;
use crate::domain::url_host::same_origin;
use crate::services::ports::SettingsStore;

/// Per-request timeout of the normal API calls.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Called before each retry sleep with `(attempt about to run, max attempts, delay seconds)`.
pub type RetryListener = Arc<dyn Fn(u32, u32, f64) + Send + Sync>;

/// Exponential backoff: `base_delay * 3^n` before retry `n + 1`.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
        }
    }
}

pub struct RommHttp {
    settings: SettingsStore,
    user_agent: String,
    secure: Client,
    insecure: Client,
    retry: RetryPolicy,
    on_retry: Option<RetryListener>,
}

impl RommHttp {
    pub fn new(
        settings: SettingsStore,
        user_agent: String,
        retry: RetryPolicy,
        on_retry: Option<RetryListener>,
    ) -> anyhow::Result<Self> {
        let secure = Client::builder().build()?;
        // The user opts into this for self-signed certs (with a UI warning).
        let insecure = Client::builder().danger_accept_invalid_certs(true).build()?;
        Ok(Self {
            settings,
            user_agent,
            secure,
            insecure,
            retry,
            on_retry,
        })
    }

    /// `Bearer <token>`, `None` when no token is stored, or a mismatch error
    /// when the token's origin is not the configured URL's origin. A legacy
    /// token without a recorded origin is still attached.
    pub fn auth_header(&self) -> Result<Option<String>, RommError> {
        let Some(token) = self.settings.get_str("romm_api_token").filter(|t| !t.is_empty()) else {
            return Ok(None);
        };
        if let Some(origin) = self.settings.get_str("romm_api_token_origin") {
            let url = self.settings.get_str("romm_url");
            if !same_origin(Some(&origin), url.as_deref()) {
                return Err(RommError::TokenHostMismatch);
            }
        }
        Ok(Some(format!("Bearer {token}")))
    }

    fn url(&self, path: &str) -> String {
        let base = self.settings.get_str("romm_url").unwrap_or_default();
        format!("{}{}", base.trim_end_matches('/'), path)
    }

    fn client(&self) -> &Client {
        if self.settings.truthy("romm_allow_insecure_ssl") {
            &self.insecure
        } else {
            &self.secure
        }
    }

    fn request(&self, method: Method, url: &str, timeout: Duration) -> RequestBuilder {
        self.client()
            .request(method, url)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .timeout(timeout)
    }

    fn authed(&self, method: Method, url: &str, timeout: Duration) -> Result<RequestBuilder, RommError> {
        let mut req = self.request(method, url, timeout);
        if let Some(header) = self.auth_header()? {
            req = req.header(reqwest::header::AUTHORIZATION, header);
        }
        Ok(req)
    }

    /// Authenticated `GET` of a JSON resource, retried on transient failures.
    pub async fn get_json(&self, path: &str) -> Result<Value, RommError> {
        let url = self.url(path);
        self.with_retry(|| async {
            let resp = send(self.authed(Method::GET, &url, REQUEST_TIMEOUT)?, &url, "GET").await?;
            json_body(resp, &url, "GET", false).await
        })
        .await
    }

    /// Authenticated `GET` in a single attempt with a short timeout.
    pub async fn get_json_once(&self, path: &str, timeout: Duration) -> Result<Value, RommError> {
        let url = self.url(path);
        let resp = send(self.authed(Method::GET, &url, timeout)?, &url, "GET").await?;
        json_body(resp, &url, "GET", false).await
    }

    /// `POST` JSON to a public endpoint: no `Authorization`, no retry (the body
    /// is a single-use credential). Error responses carry their `detail`.
    pub async fn unauthenticated_post_json(&self, path: &str, body: &Value) -> Result<Value, RommError> {
        let url = self.url(path);
        let req = self.request(Method::POST, &url, REQUEST_TIMEOUT).json(body);
        let resp = req.send().await.map_err(|e| translate_transport(&e, &url))?;
        let status = resp.status();
        if !status.is_success() {
            let detail = resp
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v.get("detail").and_then(Value::as_str).map(str::to_string));
            return Err(translate_status(status, &url, "POST", detail));
        }
        json_body(resp, &url, "POST", true).await
    }

    /// A one-off Basic-authenticated request (token mint/delete). Never
    /// retried: a duplicate mint would orphan a token.
    pub async fn basic_auth_request(
        &self,
        path: &str,
        username: &str,
        password: &str,
        method: Method,
        body: Option<&Value>,
    ) -> Result<Value, RommError> {
        let url = self.url(path);
        let label = method.as_str().to_string();
        let mut req = self
            .request(method, &url, REQUEST_TIMEOUT)
            .basic_auth(username, Some(password));
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = send(req, &url, &label).await?;
        json_body(resp, &url, &label, true).await
    }

    async fn with_retry<F, Fut>(&self, mut attempt_fn: F) -> Result<Value, RommError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Value, RommError>>,
    {
        let max = self.retry.max_attempts.max(1);
        let mut attempt = 0;
        loop {
            match attempt_fn().await {
                Ok(v) => return Ok(v),
                Err(e) if attempt + 1 < max && e.is_retryable() => {
                    let delay = self.retry.base_delay * 3u32.pow(attempt);
                    tracing::info!("Retry {}/{max} after {:?}: {e}", attempt + 1, delay);
                    if let Some(listener) = &self.on_retry {
                        listener(attempt + 2, max, delay.as_secs_f64());
                    }
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

async fn send(req: RequestBuilder, url: &str, method: &str) -> Result<Response, RommError> {
    let resp = req.send().await.map_err(|e| translate_transport(&e, url))?;
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        Err(translate_status(status, url, method, None))
    }
}

/// Parse a success body as JSON. With `empty_ok`, a 204 or empty body is `{}`.
async fn json_body(resp: Response, url: &str, method: &str, empty_ok: bool) -> Result<Value, RommError> {
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| translate_transport(&e, url))?;
    if empty_ok && (status == StatusCode::NO_CONTENT || bytes.is_empty()) {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| RommError::Api(format!("Unexpected error: invalid JSON from {method} {url}: {e}")))
}

/// Map an HTTP error status onto the error taxonomy.
pub fn translate_status(status: StatusCode, url: &str, method: &str, detail: Option<String>) -> RommError {
    let code = status.as_u16();
    let message = format!(
        "HTTP {code}: {} ({method} {url})",
        status.canonical_reason().unwrap_or("")
    );
    match code {
        400 => RommError::Api(format!("Bad request ({method} {url})")),
        401 => RommError::Auth(message),
        403 => RommError::Forbidden(message),
        404 => RommError::NotFound { message, detail },
        429 => RommError::Server {
            status: 429,
            message: format!("Rate limited \u{2014} too many requests ({method} {url})"),
        },
        500.. => RommError::Server { status: code, message },
        _ => RommError::Api(message),
    }
}

/// Map a transport failure (no HTTP status) onto the error taxonomy.
pub fn translate_transport(err: &reqwest::Error, url: &str) -> RommError {
    let mut chain = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(s) = source {
        chain.push_str(": ");
        chain.push_str(&s.to_string());
        source = s.source();
    }
    let lower = chain.to_ascii_lowercase();
    if err.is_timeout() {
        RommError::Timeout(chain)
    } else if lower.contains("certificate") || lower.contains("tls") || lower.contains("handshake") {
        RommError::Ssl(chain)
    } else if err.is_builder() {
        RommError::Api(format!("Unexpected error: {chain} ({url})"))
    } else {
        RommError::Connection(chain)
    }
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
