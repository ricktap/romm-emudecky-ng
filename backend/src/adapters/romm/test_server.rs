//! A scriptable raw-TCP HTTP/1.1 server for transfer tests.
//!
//! Wiremock cannot drop a connection mid-body, stall after N bytes or drip a
//! body slowly, which are exactly the failures a multipart download must
//! survive. A handler maps `(request, request index)` to a [`Reply`]; every
//! request is recorded and the peak number of simultaneously open connections
//! is tracked (to prove a job never has two workers).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone)]
pub struct Req {
    pub path: String,
    /// Lower-cased header names.
    pub headers: HashMap<String, String>,
}

impl Req {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

#[derive(Debug, Clone)]
pub enum Body {
    Full,
    /// Write this many bytes, then close the connection (a cut-off transfer).
    AbortAfter(usize),
    /// Write this many bytes, then go silent without closing.
    StallAfter(usize),
    /// Write `chunk` bytes every `every` (a slow but healthy transfer).
    Drip { chunk: usize, every: Duration },
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Silence before the status line (RomM preparing an archive).
    pub header_delay: Duration,
    pub body: Vec<u8>,
    pub body_plan: Body,
    /// Close the connection without answering.
    pub drop_connection: bool,
}

impl Reply {
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            header_delay: Duration::ZERO,
            body,
            body_plan: Body::Full,
            drop_connection: false,
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn delayed(mut self, d: Duration) -> Self {
        self.header_delay = d;
        self
    }

    pub fn plan(mut self, plan: Body) -> Self {
        self.body_plan = plan;
        self
    }

    pub fn dropped() -> Self {
        let mut r = Self::new(0, Vec::new());
        r.drop_connection = true;
        r
    }
}

type Handler = Arc<dyn Fn(&Req, usize) -> Reply + Send + Sync>;

pub struct TestServer {
    pub url: String,
    requests: Arc<Mutex<Vec<Req>>>,
    peak: Arc<AtomicUsize>,
    handler: Arc<Mutex<Handler>>,
}

impl TestServer {
    pub async fn start(handler: impl Fn(&Req, usize) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Req>>> = Arc::default();
        let peak = Arc::new(AtomicUsize::new(0));
        let open = Arc::new(AtomicUsize::new(0));
        let handler: Arc<Mutex<Handler>> = Arc::new(Mutex::new(Arc::new(handler)));
        let (reqs, pk, op, h) = (requests.clone(), peak.clone(), open.clone(), handler.clone());
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let now = op.fetch_add(1, Ordering::SeqCst) + 1;
                pk.fetch_max(now, Ordering::SeqCst);
                let (reqs, op, h) = (reqs.clone(), op.clone(), h.clone());
                tokio::spawn(async move {
                    serve(sock, reqs, h).await;
                    op.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Self { url, requests, peak, handler }
    }

    /// Swap the behaviour (a "restarted" server answering differently).
    pub fn set_handler(&self, handler: impl Fn(&Req, usize) -> Reply + Send + Sync + 'static) {
        *self.handler.lock().unwrap() = Arc::new(handler);
    }

    pub fn requests(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }

    /// The most connections that were ever open at the same time.
    pub fn peak_connections(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        206 => "Partial Content",
        401 => "Unauthorized",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

async fn serve(mut sock: TcpStream, requests: Arc<Mutex<Vec<Req>>>, handler: Arc<Mutex<Handler>>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match sock.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let mut lines = head.split("\r\n");
    let path = lines.next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let req = Req { path, headers };
    let reply = {
        let mut seen = requests.lock().unwrap();
        let index = seen.len();
        seen.push(req.clone());
        let h = handler.lock().unwrap().clone();
        h(&req, index)
    };
    if reply.drop_connection {
        return;
    }
    if !reply.header_delay.is_zero() {
        tokio::time::sleep(reply.header_delay).await;
    }
    let mut out = format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status));
    for (k, v) in &reply.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!("content-length: {}\r\nconnection: close\r\n\r\n", reply.body.len()));
    if sock.write_all(out.as_bytes()).await.is_err() {
        return;
    }
    match reply.body_plan {
        Body::Full => {
            let _ = sock.write_all(&reply.body).await;
        }
        Body::AbortAfter(n) => {
            let _ = sock.write_all(&reply.body[..n.min(reply.body.len())]).await;
            let _ = sock.flush().await;
            // dropping the socket here closes it before content-length is reached
        }
        Body::StallAfter(n) => {
            let _ = sock.write_all(&reply.body[..n.min(reply.body.len())]).await;
            let _ = sock.flush().await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
        Body::Drip { chunk, every } => {
            for piece in reply.body.chunks(chunk) {
                if sock.write_all(piece).await.is_err() {
                    return;
                }
                let _ = sock.flush().await;
                tokio::time::sleep(every).await;
            }
        }
    }
    let _ = sock.flush().await;
}

/// How RomM answers for one cached ZIP: `Range` serves the static cached file
/// (`206`, `ETag`, `Last-Modified`), `If-Range` with another tag gets the whole
/// representation (`200`), an out-of-range start gets `416`.
pub fn cached_zip_reply(req: &Req, zip: &[u8], etag: &str, last_modified: &str) -> Reply {
    let validators = |r: Reply| r.header("etag", etag).header("last-modified", last_modified).header("accept-ranges", "bytes");
    let Some(range) = req.header("range") else {
        // No Range: RomM streams a generated ZIP (no validators, not byte-identical).
        return Reply::new(200, zip.to_vec());
    };
    let start: usize = range
        .strip_prefix("bytes=")
        .and_then(|r| r.trim_end_matches('-').parse().ok())
        .unwrap_or(0);
    if req.header("if-range").is_some_and(|v| v != etag) {
        return validators(Reply::new(200, zip.to_vec()));
    }
    if start >= zip.len() {
        return Reply::new(416, Vec::new()).header("content-range", &format!("bytes */{}", zip.len()));
    }
    validators(
        Reply::new(206, zip[start..].to_vec())
            .header("content-range", &format!("bytes {start}-{}/{}", zip.len() - 1, zip.len())),
    )
}
