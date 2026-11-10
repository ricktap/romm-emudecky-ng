//! HTTP transport for the RomM API: headers, token-host binding, TLS mode,
//! error translation and the retry policy.
//!
//! Every request carries the plugin `User-Agent` (Cloudflare's Bot Fight Mode
//! rejects anonymous clients). The bearer token is attached only when its
//! minting origin (`romm_api_token_origins`, or the single legacy
//! `romm_api_token_origin`) matches the URL the request goes to; a mismatch
//! refuses the request with [`RommError::TokenHostMismatch`] before any byte leaves.
//!
//! With both `romm_url` (internal) and `romm_url_external` set, the internal one
//! is used while its host accepts a TCP connection within a few seconds; only a
//! genuine connect failure falls back to the external URL. The winner is cached
//! for a short time and re-probed after a transient failure.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::HeaderMap;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;

use crate::domain::archive_transfer::{ResponseHead, ResumePlan, ResumeVerdict, judge_fresh, judge_resume};
use crate::domain::download_failure::{ArchiveRecoveryNeeded, TransferCancelled};
use crate::domain::errors::RommError;
use crate::domain::url_host::same_origin;
use crate::services::ports::{
    IdentitySink, SettingsStore, TransferHooks, TransferMode, TransferOutcome, TransferPhase,
};
use crate::util::path_safety::quote;

/// Per-request timeout of the normal API calls.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The three deadlines of a transfer, kept apart on purpose:
///
/// * `connect` bounds TCP + TLS setup only (reqwest's `connect_timeout`).
/// * `headers_*` bound the wait for the response headers after the request is
///   out. For a multipart ROM that wait can be RomM building a large archive
///   (minutes), so it is far longer than for a single file.
/// * `body_inactivity` bounds silence *inside* a connected body. There is
///   deliberately no total-request deadline: a healthy transfer may run for hours.
#[derive(Clone, Copy, Debug)]
pub struct TransferTimeouts {
    pub connect: Duration,
    pub headers_single: Duration,
    pub headers_archive: Duration,
    pub body_inactivity: Duration,
}

impl Default for TransferTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            headers_single: Duration::from_secs(30),
            headers_archive: Duration::from_secs(15 * 60),
            body_inactivity: Duration::from_secs(60),
        }
    }
}

/// Longest pause between two attempts of one transfer.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

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

/// How long the chosen base URL is trusted before the internal one is probed again.
const BASE_TTL: Duration = Duration::from_secs(60);
/// A connect failure is decided within this long (a few seconds, never a short read timeout).
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub struct RommHttp {
    /// The base URL last chosen between internal and external, and when.
    base_cache: Mutex<Option<(String, Instant)>>,
    settings: SettingsStore,
    user_agent: String,
    secure: Client,
    insecure: Client,
    retry: RetryPolicy,
    on_retry: Option<RetryListener>,
    timeouts: TransferTimeouts,
}

impl RommHttp {
    pub fn new(
        settings: SettingsStore,
        user_agent: String,
        retry: RetryPolicy,
        on_retry: Option<RetryListener>,
    ) -> anyhow::Result<Self> {
        let timeouts = TransferTimeouts::default();
        let (secure, insecure) = Self::clients(&timeouts)?;
        Ok(Self {
            base_cache: Mutex::new(None),
            settings,
            user_agent,
            secure,
            insecure,
            retry,
            on_retry,
            timeouts,
        })
    }

    fn clients(timeouts: &TransferTimeouts) -> anyhow::Result<(Client, Client)> {
        let secure = Client::builder().connect_timeout(timeouts.connect).build()?;
        // The user opts into this for self-signed certs (with a UI warning).
        let insecure = Client::builder()
            .connect_timeout(timeouts.connect)
            .danger_accept_invalid_certs(true)
            .build()?;
        Ok((secure, insecure))
    }

    /// Use other transfer deadlines (tests shorten them).
    pub fn with_timeouts(mut self, timeouts: TransferTimeouts) -> anyhow::Result<Self> {
        let (secure, insecure) = Self::clients(&timeouts)?;
        self.secure = secure;
        self.insecure = insecure;
        self.timeouts = timeouts;
        Ok(self)
    }

    /// `Bearer <token>`, `None` when no token is stored, or a mismatch error
    /// when the token's origin is not the configured URL's origin. A legacy
    /// token without a recorded origin is still attached.
    pub fn auth_header(&self, url: &str) -> Result<Option<String>, RommError> {
        let Some(token) = self.settings.get_str("romm_api_token").filter(|t| !t.is_empty()) else {
            return Ok(None);
        };
        let origins: Vec<String> = match self.settings.get("romm_api_token_origins") {
            Some(Value::Array(list)) if !list.is_empty() => {
                list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
            }
            _ => self.settings.get_str("romm_api_token_origin").into_iter().collect(),
        };
        // The bearer goes only to one of the origins it was minted for (none recorded: a legacy token).
        if !origins.is_empty() && !origins.iter().any(|o| same_origin(Some(o), Some(url))) {
            return Err(RommError::TokenHostMismatch);
        }
        Ok(Some(format!("Bearer {token}")))
    }

    /// The base URL requests go to now: the internal URL, or the external one when the internal host
    /// cannot be connected to. Cached for [`BASE_TTL`].
    async fn base(&self) -> String {
        let internal = self.settings.get_str("romm_url").unwrap_or_default().trim().to_string();
        let external = self.settings.get_str("romm_url_external").unwrap_or_default().trim().to_string();
        if external.is_empty() {
            return internal;
        }
        if internal.is_empty() {
            return external;
        }
        if let Some((base, at)) = self.base_cache.lock().unwrap().clone()
            && at.elapsed() < BASE_TTL
            && (base == internal || base == external)
        {
            return base;
        }
        let winner = if can_connect(&internal).await { internal } else { external };
        *self.base_cache.lock().unwrap() = Some((winner.clone(), Instant::now()));
        winner
    }

    /// Forget the chosen base so the next request probes again (after a transient failure).
    fn forget_base(&self) {
        *self.base_cache.lock().unwrap() = None;
    }

    async fn url(&self, path: &str) -> String {
        let base = self.base().await;
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
        if let Some(header) = self.auth_header(url)? {
            req = req.header(reqwest::header::AUTHORIZATION, header);
        }
        Ok(req)
    }

    /// Authenticated `GET` of a JSON resource, retried on transient failures.
    pub async fn get_json(&self, path: &str) -> Result<Value, RommError> {
        let url = self.url(path).await;
        self.with_retry(|| async {
            let resp = send(self.authed(Method::GET, &url, REQUEST_TIMEOUT)?, &url, "GET").await?;
            json_body(resp, &url, "GET", false).await
        })
        .await
    }

    /// Authenticated JSON `POST`/`PUT`, one attempt (a write is never replayed blindly). An empty answer is `{}`.
    pub async fn send_json(&self, method: Method, path: &str, body: &Value) -> Result<Value, RommError> {
        let url = self.url(path).await;
        let label = method.as_str().to_string();
        let resp = send(self.authed(method, &url, REQUEST_TIMEOUT)?.json(body), &url, &label).await?;
        json_body(resp, &url, &label, true).await
    }

    /// Authenticated multipart upload of one in-memory file under the form field `saveFile` (RomM's save endpoints). One
    /// attempt, a longer timeout than a plain request; the file name never carries a line break, NUL or quote.
    pub async fn upload_save_file(&self, method: Method, path: &str, file_name: &str, data: Vec<u8>) -> Result<Value, RommError> {
        let url = self.url(path).await;
        let label = method.as_str().to_string();
        let safe: String = file_name.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0' | '"')).collect();
        let part = reqwest::multipart::Part::bytes(data).file_name(safe).mime_str("application/octet-stream").map_err(|e| RommError::Api(e.to_string()))?;
        let form = reqwest::multipart::Form::new().part("saveFile", part);
        let resp = send(self.authed(method, &url, Duration::from_secs(60))?.multipart(form), &url, &label).await?;
        json_body(resp, &url, &label, true).await
    }

    /// Authenticated `GET` of a binary body held in memory (a save is small), refused beyond `limit` bytes.
    pub async fn get_bytes(&self, path: &str, limit: u64) -> Result<Vec<u8>, RommError> {
        let url = self.url(path).await;
        self.with_retry(|| async {
            let resp = send(self.authed(Method::GET, &url, Duration::from_secs(60))?, &url, "GET").await?;
            if resp.content_length().is_some_and(|n| n > limit) {
                return Err(RommError::Api(format!("Response too large ({url})")));
            }
            let bytes = resp.bytes().await.map_err(|e| translate_transport(&e, &url))?;
            if bytes.len() as u64 > limit {
                return Err(RommError::Api(format!("Response too large ({url})")));
            }
            Ok(bytes.to_vec())
        })
        .await
    }

    /// Authenticated `GET` in a single attempt with a short timeout.
    pub async fn get_json_once(&self, path: &str, timeout: Duration) -> Result<Value, RommError> {
        let url = self.url(path).await;
        let resp = send(self.authed(Method::GET, &url, timeout)?, &url, "GET").await?;
        json_body(resp, &url, "GET", false).await
    }

    /// Native play-session ingest is never retried here; the durable outbox
    /// owns retries and needs the structured FastAPI 422 detail intact.
    pub async fn post_json_once(&self,path:&str,body:&Value)->Result<Value,crate::services::ports::PlaytimeApiError>{
        use crate::services::ports::PlaytimeApiError;
        let url=self.url(path).await;
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
        let url = self.url(path).await;
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
        let url = self.url(path).await;
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

    async fn with_retry<T, F, Fut>(&self, mut attempt_fn: F) -> Result<T, RommError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, RommError>>,
    {
        let max = self.retry.max_attempts.max(1);
        let mut attempt = 0;
        loop {
            match attempt_fn().await {
                Ok(v) => return Ok(v),
                Err(e) if attempt + 1 < max && e.is_retryable() => {
                    self.forget_base();
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

/// Whether the host of `base` accepts a TCP connection within [`PROBE_CONNECT_TIMEOUT`]. Only a genuine connect
/// failure counts: a slow or erroring server that did accept the connection is still the right one.
async fn can_connect(base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base) else {
        return false;
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    matches!(
        tokio::time::timeout(PROBE_CONNECT_TIMEOUT, tokio::net::TcpStream::connect((host, port))).await,
        Ok(Ok(_))
    )
}

/// The caller's callbacks, borrowed for the duration of one transfer.
struct Sink<'a> {
    progress: &'a mut (dyn FnMut(u64, u64) + Send),
    on_meta: &'a mut Option<Box<dyn FnOnce(bool) + Send>>,
    on_phase: &'a mut (dyn FnMut(TransferPhase) + Send),
    on_identity: &'a mut Option<IdentitySink>,
    cancel: &'a tokio_util::sync::CancellationToken,
}

impl Sink<'_> {
    fn meta(&mut self, range_supported: bool) {
        if let Some(meta) = self.on_meta.take() {
            meta(range_supported);
        }
    }
}

fn file_len_sync(path: &str) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// Spread retries of many jobs a little (+-20%) so they do not hit a recovering
/// server in lockstep. A zero delay stays zero.
fn jittered(delay: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    delay.mul_f64(0.8 + f64::from(nanos % 400) / 1000.0)
}

impl RommHttp {
    /// Pause before attempt `attempt + 2`, or give the error back. Gives up when
    /// the attempts are used up, the error is not transient, or the transfer
    /// was cancelled; a cancel during the pause ends it at once.
    async fn backoff_or_give_up(
        &self,
        err: anyhow::Error,
        attempt: u32,
        max: u32,
        sink: &mut Sink<'_>,
    ) -> anyhow::Result<()> {
        let retryable = err.downcast_ref::<RommError>().is_some_and(RommError::is_retryable);
        if attempt + 1 >= max || !retryable || sink.cancel.is_cancelled() {
            return Err(err);
        }
        let delay = jittered((self.retry.base_delay * 3u32.pow(attempt)).min(MAX_BACKOFF));
        tracing::info!("Retry {}/{max} after {:?}: {err}", attempt + 1, delay);
        if let Some(listener) = &self.on_retry {
            listener(attempt + 2, max, delay.as_secs_f64());
        }
        (sink.on_phase)(TransferPhase::Retrying {
            attempt: attempt + 2,
            max_attempts: max,
            delay_secs: delay.as_secs_f64(),
        });
        tokio::select! {
            _ = sink.cancel.cancelled() => Err(TransferCancelled.into()),
            _ = tokio::time::sleep(delay) => Ok(()),
        }
    }

    /// Stream `path` into the file `dest`, retried on transient failures.
    ///
    /// * [`TransferMode::Single`]: a plain `GET`; with `resume` and a partial in
    ///   `dest` the request carries `Range: bytes=<size>-` and the live response
    ///   decides: a `206` whose `Content-Range` starts at the partial's size
    ///   appends, anything else restarts from byte 0.
    /// * [`TransferMode::Archive`]: a multipart ROM as RomM's cached ZIP. The
    ///   request always carries `Range` (so the first and every later transfer
    ///   see the same cached representation) and `Accept-Encoding: identity`;
    ///   a resume adds `If-Range` with the saved strong `ETag`, and a response
    ///   is checked against the saved identity before a byte is appended. A
    ///   `200` is never appended: it replaces the partial once accepted.
    ///
    /// `on_meta` fires once, when the first accepted response's headers arrive,
    /// with whether the transfer is resumable. The written length is checked
    /// against the full size. Local write failures keep their `io::Error` (so a
    /// full disk is recognisable) under a `Connection` error. A cancelled
    /// `hooks.cancel` stops the transfer with [`TransferCancelled`], keeping the
    /// bytes already written.
    pub async fn download(
        &self,
        path: &str,
        dest: &str,
        mode: TransferMode,
        hooks: TransferHooks,
    ) -> anyhow::Result<TransferOutcome> {
        let url = self.url(&quote(path, "/:?=&@")).await;
        if let Some(parent) = std::path::Path::new(dest).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| local_io(e, "creating the download directory"))?;
        }
        let TransferHooks {
            mut progress,
            mut on_meta,
            mut on_phase,
            mut on_identity,
            cancel,
        } = hooks;
        let mut sink = Sink {
            progress: &mut *progress,
            on_meta: &mut on_meta,
            on_phase: &mut *on_phase,
            on_identity: &mut on_identity,
            cancel: &cancel,
        };
        let max = self.retry.max_attempts.max(1);
        let mut attempt = 0;
        match mode {
            TransferMode::Single { resume } => loop {
                (sink.on_phase)(TransferPhase::Waiting { attempt: attempt + 1, max_attempts: max });
                match self.download_once(&url, dest, resume, &mut sink).await {
                    Ok(()) => return Ok(TransferOutcome::Downloaded),
                    Err(err) => self.backoff_or_give_up(err, attempt, max, &mut sink).await?,
                }
                attempt += 1;
            },
            TransferMode::Archive(mut plan) => loop {
                if matches!(plan, ResumePlan::NeedsRecovery) {
                    return Err(ArchiveRecoveryNeeded("a partial of unknown origin".into()).into());
                }
                (sink.on_phase)(TransferPhase::Waiting { attempt: attempt + 1, max_attempts: max });
                match self.archive_attempt(&url, dest, &mut plan, &mut sink).await {
                    Ok(outcome) => return Ok(outcome),
                    Err(err) => self.backoff_or_give_up(err, attempt, max, &mut sink).await?,
                }
                attempt += 1;
            },
        }
    }

    /// Send `req`, waiting at most `deadline` for the response headers.
    async fn await_headers(
        &self,
        req: RequestBuilder,
        deadline: Duration,
        url: &str,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<Response> {
        let sent = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(TransferCancelled.into()),
            r = tokio::time::timeout(deadline, req.send()) => r,
        };
        match sent {
            Err(_) => Err(RommError::Timeout(format!(
                "timed out after {}s waiting for RomM to answer ({url})",
                deadline.as_secs()
            ))
            .into()),
            Ok(Err(e)) => Err(translate_transport(&e, url).into()),
            Ok(Ok(resp)) => Ok(resp),
        }
    }

    fn base_request(&self, url: &str) -> anyhow::Result<RequestBuilder> {
        let mut req = self.client().get(url).header(reqwest::header::USER_AGENT, &self.user_agent);
        if let Some(header) = self.auth_header(url)? {
            req = req.header(reqwest::header::AUTHORIZATION, header);
        }
        Ok(req)
    }

    /// One attempt at a multipart ROM. `plan` is updated as responses are
    /// accepted so that a retry inside the same call continues the same
    /// cached representation from the actual size on disk.
    async fn archive_attempt(
        &self,
        url: &str,
        dest: &str,
        plan: &mut ResumePlan,
        sink: &mut Sink<'_>,
    ) -> anyhow::Result<TransferOutcome> {
        if sink.cancel.is_cancelled() {
            return Err(TransferCancelled.into());
        }
        // The actual file is the truth; a stale plan never overrides it.
        let local = file_len_sync(dest);
        let append = match &*plan {
            ResumePlan::Append(id) if local > 0 && local <= id.length => Some(id.clone()),
            _ => None,
        };
        let mut req = self
            .base_request(url)?
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        req = match &append {
            Some(id) => req
                .header(reqwest::header::RANGE, format!("bytes={local}-"))
                .header(reqwest::header::IF_RANGE, &id.etag),
            None => req.header(reqwest::header::RANGE, "bytes=0-"),
        };
        let resp = self
            .await_headers(req, self.timeouts.headers_archive, url, sink.cancel)
            .await?;
        let status = resp.status();
        let headers = resp.headers();
        let head = ResponseHead {
            status: status.as_u16(),
            content_range: parse_content_range(header(headers, "content-range")),
            content_length: header(headers, "content-length").and_then(|v| v.trim().parse().ok()),
            etag: header(headers, "etag"),
            last_modified: header(headers, "last-modified"),
            content_encoding: header(headers, "content-encoding"),
            content_type: header(headers, "content-type"),
        };
        if status == StatusCode::RANGE_NOT_SATISFIABLE
            && let Some(id) = &append
        {
            // Only a local file with the saved complete length can be done; the
            // caller validates the archive before believing it.
            return match judge_resume(&head, local, id) {
                Ok(ResumeVerdict::NotSatisfiable) if local == id.length => Ok(TransferOutcome::AlreadyComplete),
                _ => Err(ArchiveRecoveryNeeded(format!(
                    "RomM refused the range from byte {local} but the saved archive is {} bytes",
                    id.length
                ))
                .into()),
            };
        }
        if !status.is_success() {
            return Err(translate_status(status, url, "GET", None).into());
        }
        let accepted = match &append {
            Some(id) => match judge_resume(&head, local, id) {
                Ok(ResumeVerdict::Append) => None,
                Ok(ResumeVerdict::Replace(fresh)) => Some(fresh),
                Ok(ResumeVerdict::NotSatisfiable) => unreachable!("416 handled above"),
                Err(rejection) => return Err(ArchiveRecoveryNeeded(rejection.to_string()).into()),
            },
            None => Some(judge_fresh(&head).map_err(|rejection| {
                RommError::Connection(format!("Unexpected response from RomM: {rejection}"))
            })?),
        };
        match accepted {
            // A consistent 206 at the local offset: append.
            None => {
                let id = append.expect("an append verdict needs an identity");
                sink.meta(true);
                (sink.on_phase)(TransferPhase::Transferring);
                let file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(dest)
                    .await
                    .map_err(|e| local_io(e, "opening the download file"))?;
                self.stream_body(resp, file, local, id.length, url, sink).await?;
                Ok(TransferOutcome::Downloaded)
            }
            // A whole representation (first request, or the validator changed):
            // replace the partial now that the response is accepted, save its
            // identity, and only then write bytes.
            Some(fresh) => {
                sink.meta(fresh.cached);
                (sink.on_phase)(TransferPhase::Transferring);
                let file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(dest)
                    .await
                    .map_err(|e| local_io(e, "opening the download file"))?;
                if let Some(save) = sink.on_identity.as_mut() {
                    save(fresh.identity.clone())
                        .await
                        .map_err(|e| e.context("saving the archive identity before downloading"))?;
                }
                *plan = match &fresh.identity {
                    Some(identity) => ResumePlan::Append(identity.clone()),
                    None => ResumePlan::Fresh,
                };
                self.stream_body(resp, file, 0, fresh.total, url, sink).await?;
                Ok(TransferOutcome::Downloaded)
            }
        }
    }

    async fn download_once(&self, url: &str, dest: &str, resume: bool, sink: &mut Sink<'_>) -> anyhow::Result<()> {
        if sink.cancel.is_cancelled() {
            return Err(TransferCancelled.into());
        }
        let existing = if resume {
            tokio::fs::metadata(dest).await.map_or(0, |m| m.len())
        } else {
            0
        };
        let mut req = self.base_request(url)?;
        if existing > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let resp = self
            .await_headers(req, self.timeouts.headers_single, url, sink.cancel)
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(translate_status(status, url, "GET", None).into());
        }
        sink.meta(range_supported(status, resp.headers()));
        (sink.on_phase)(TransferPhase::Transferring);
        let (append, downloaded, total) = resume_branch(status, resp.headers(), existing);
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(dest)
            .await
            .map_err(|e| local_io(e, "opening the download file"))?;
        self.stream_body(resp, file, downloaded, total, url, sink).await
    }

    /// Write the body to `file`, reporting progress. Silence for longer than the
    /// inactivity deadline is a stall; there is no total deadline. Whatever
    /// happens, the bytes written so far stay on disk for a resume.
    async fn stream_body(
        &self,
        mut resp: Response,
        mut file: tokio::fs::File,
        mut downloaded: u64,
        total: u64,
        url: &str,
        sink: &mut Sink<'_>,
    ) -> anyhow::Result<()> {
        let inactivity = self.timeouts.body_inactivity;
        let streamed: anyhow::Result<()> = async {
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = sink.cancel.cancelled() => return Err(TransferCancelled.into()),
                    c = tokio::time::timeout(inactivity, resp.chunk()) => c,
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
                    (sink.progress)(downloaded, total);
                }
            }
        }
        .await;
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
        409 => RommError::Conflict(message),
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

#[cfg(test)]
#[path = "archive_download_tests.rs"]
mod archive_download_tests;
