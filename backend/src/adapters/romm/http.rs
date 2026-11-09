//! HTTP transport for the RomM API: headers, token-host binding, TLS mode,
//! error translation and the retry policy.
//!
//! Every request carries the plugin `User-Agent` (Cloudflare's Bot Fight Mode
//! rejects anonymous clients). The bearer token is attached only when its
//! minting origin matches the configured `romm_url`; a mismatch refuses the
//! request with [`RommError::TokenHostMismatch`] before any byte leaves.

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;

use crate::domain::download_failure::TransferCancelled;
use crate::domain::errors::RommError;
use crate::domain::url_host::same_origin;
use crate::services::ports::{SettingsStore, TransferHooks};
use crate::util::path_safety::quote;

/// Per-request timeout of the normal API calls.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a download waits for the response headers.
pub const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a download waits for the next body chunk before it counts as stalled.
pub const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(60);

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

    /// Native play-session ingest is never retried here; the durable outbox
    /// owns retries and needs the structured FastAPI 422 detail intact.
    pub async fn post_json_once(&self,path:&str,body:&Value)->Result<Value,crate::services::ports::PlaytimeApiError>{
        use crate::services::ports::PlaytimeApiError;
        let url=self.url(path);
        let response=self.authed(Method::POST,&url,REQUEST_TIMEOUT)?.json(body).send().await.map_err(|e|translate_transport(&e,&url))?;
        let status=response.status();
        if status.as_u16()==422{
            let value=response.json::<Value>().await.unwrap_or(Value::Null);
            return Err(PlaytimeApiError::Rejected(value["detail"].clone()));
        }
        if !status.is_success(){return Err(translate_status(status,&url,"POST",None).into());}
        Ok(json_body(response,&url,"POST",false).await?)
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

impl RommHttp {
    /// Stream `path` into the file `dest`, retried on transient failures.
    ///
    /// With `resume` and a partial already in `dest`, the request carries
    /// `Range: bytes=<size>-` and the live response decides: a `206` whose
    /// `Content-Range` starts at the partial's size appends; any other answer
    /// (a `200` because the server or a Cloudflare edge ignored the Range, or
    /// a `206` at the wrong offset) restarts from byte 0. `on_meta` fires once,
    /// when the first response's headers arrive, with whether the transfer is
    /// resumable. The written length is checked against the full size: a short
    /// stream or an overshoot fails as `Download incomplete`, an empty unsized
    /// body as `Download produced 0 bytes`. Local write failures keep their
    /// `io::Error` (so a full disk is recognisable) under a `Connection` error.
    /// A cancelled `hooks.cancel` stops the transfer with [`TransferCancelled`],
    /// keeping the bytes already written.
    pub async fn download(&self, path: &str, dest: &str, resume: bool, hooks: TransferHooks) -> anyhow::Result<()> {
        let url = self.url(&quote(path, "/:?=&@"));
        if let Some(parent) = std::path::Path::new(dest).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| local_io(e, "creating the download directory"))?;
        }
        let TransferHooks {
            mut progress,
            mut on_meta,
            cancel,
        } = hooks;
        let max = self.retry.max_attempts.max(1);
        let mut attempt = 0;
        loop {
            let result = self
                .download_once(&url, dest, resume, &mut *progress, &mut on_meta, &cancel)
                .await;
            let err = match result {
                Ok(()) => return Ok(()),
                Err(e) => e,
            };
            let retryable = err.downcast_ref::<RommError>().is_some_and(RommError::is_retryable);
            if attempt + 1 >= max || !retryable || cancel.is_cancelled() {
                return Err(err);
            }
            let delay = self.retry.base_delay * 3u32.pow(attempt);
            tracing::info!("Retry {}/{max} after {:?}: {err}", attempt + 1, delay);
            if let Some(listener) = &self.on_retry {
                listener(attempt + 2, max, delay.as_secs_f64());
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(TransferCancelled.into()),
                _ = tokio::time::sleep(delay) => {}
            }
            attempt += 1;
        }
    }

    async fn download_once(
        &self,
        url: &str,
        dest: &str,
        resume: bool,
        progress: &mut (dyn FnMut(u64, u64) + Send),
        on_meta: &mut Option<Box<dyn FnOnce(bool) + Send>>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
        if cancel.is_cancelled() {
            return Err(TransferCancelled.into());
        }
        let existing = if resume {
            tokio::fs::metadata(dest).await.map_or(0, |m| m.len())
        } else {
            0
        };
        let mut req = self.client().get(url).header(reqwest::header::USER_AGENT, &self.user_agent);
        if let Some(header) = self.auth_header()? {
            req = req.header(reqwest::header::AUTHORIZATION, header);
        }
        if existing > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let sent = tokio::select! {
            _ = cancel.cancelled() => return Err(TransferCancelled.into()),
            r = tokio::time::timeout(DOWNLOAD_CONNECT_TIMEOUT, req.send()) => r,
        };
        let mut resp = match sent {
            Err(_) => return Err(RommError::Timeout(format!("timed out connecting to {url}")).into()),
            Ok(Err(e)) => return Err(translate_transport(&e, url).into()),
            Ok(Ok(resp)) => resp,
        };
        let status = resp.status();
        if !status.is_success() {
            return Err(translate_status(status, url, "GET", None).into());
        }
        if let Some(meta) = on_meta.take() {
            meta(range_supported(status, resp.headers()));
        }
        let (append, mut downloaded, total) = resume_branch(status, resp.headers(), existing);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(dest)
            .await
            .map_err(|e| local_io(e, "opening the download file"))?;
        let streamed: anyhow::Result<()> = async {
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(TransferCancelled.into()),
                    c = tokio::time::timeout(DOWNLOAD_READ_TIMEOUT, resp.chunk()) => c,
                };
                let bytes = match chunk {
                    Err(_) => {
                        return Err(RommError::Timeout(
                            "Download stalled: no data received within read timeout".into(),
                        )
                        .into());
                    }
                    Ok(Err(e)) => return Err(translate_transport(&e, url).into()),
                    Ok(Ok(None)) => return Ok(()),
                    Ok(Ok(Some(bytes))) => bytes,
                };
                file.write_all(&bytes)
                    .await
                    .map_err(|e| local_io(e, "writing the download file"))?;
                downloaded += bytes.len() as u64;
                if total > 0 {
                    progress(downloaded, total);
                }
            }
        }
        .await;
        // Whatever happened, the bytes written so far stay on disk for a resume.
        let flushed = file.flush().await;
        streamed?;
        flushed.map_err(|e| local_io(e, "writing the download file"))?;
        validate_download(total, downloaded)
    }
}

/// A local filesystem failure during a transfer: a `Connection` error that
/// keeps the `io::Error` in its cause chain.
fn local_io(err: std::io::Error, what: &str) -> anyhow::Error {
    let message = format!("{what}: {err}");
    anyhow::Error::new(err).context(RommError::Connection(message))
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The response came through a Cloudflare edge (which discards `Range`).
pub fn is_cloudflare(headers: &HeaderMap) -> bool {
    header(headers, "cf-ray").is_some_and(|v| !v.is_empty())
        || header(headers, "server").is_some_and(|v| v.to_lowercase().contains("cloudflare"))
}

/// Whether the response proves the transfer can be resumed: a `206`, or
/// `Accept-Ranges: bytes`, unless a Cloudflare edge served it.
pub fn range_supported(status: StatusCode, headers: &HeaderMap) -> bool {
    let ranged = status == StatusCode::PARTIAL_CONTENT
        || header(headers, "accept-ranges").is_some_and(|v| v.eq_ignore_ascii_case("bytes"));
    ranged && !is_cloudflare(headers)
}

/// `Content-Range: bytes start-end/total` → `(start, total)`; `None` when
/// missing, malformed, or the total is unknown (`*`).
pub fn parse_content_range(value: Option<&str>) -> Option<(u64, u64)> {
    let (unit, spec) = value?.trim().split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = spec.split_once('/').unwrap_or((spec, ""));
    if total == "*" {
        return None;
    }
    let start = range.split('-').next()?.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()?))
}

/// `(append, bytes already on disk to count, full size)` from the live
/// response. Only a `206` starting exactly at the partial's size appends; a
/// `206` at another offset restarts but validates against the full size; any
/// other status restarts against `Content-Length`.
pub fn resume_branch(status: StatusCode, headers: &HeaderMap, existing: u64) -> (bool, u64, u64) {
    if status == StatusCode::PARTIAL_CONTENT
        && let Some((start, total)) = parse_content_range(header(headers, "content-range"))
    {
        if start == existing && existing > 0 {
            return (true, existing, total);
        }
        return (false, 0, total);
    }
    let length = header(headers, "content-length")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    (false, 0, length)
}

/// Fail a transfer whose length does not match its declared size, or that
/// produced nothing without declaring one.
pub fn validate_download(total: u64, downloaded: u64) -> anyhow::Result<()> {
    if total > 0 && downloaded != total {
        return Err(RommError::Connection(format!(
            "Download incomplete: got {downloaded} bytes, expected {total}"
        ))
        .into());
    }
    if total == 0 && downloaded == 0 {
        return Err(RommError::Connection(
            "Download produced 0 bytes (no Content-Length header and no data received)".into(),
        )
        .into());
    }
    Ok(())
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

#[cfg(test)]
#[path = "download_tests.rs"]
mod download_tests;
