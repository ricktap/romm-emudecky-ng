//! A minimal RomM over raw TCP for transfer tests: serves ROM details and ROM
//! content, honours `Range: bytes=N-` with a `206`, and can stall a body after
//! a given number of bytes until released (so a test can kill or pause the
//! backend mid-transfer). Every request is recorded.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub path: String,
    pub range: Option<String>,
    pub if_range: Option<String>,
    pub accept_encoding: Option<String>,
}

#[derive(Default)]
pub struct State {
    pub roms: HashMap<i64, Value>,
    pub content: HashMap<i64, Vec<u8>>,
    /// Stop writing a content body after this many bytes (of the full file).
    pub stall_at: Option<usize>,
    pub requests: Vec<Seen>,
    /// Content served as RomM's cached ZIP: with `Range` it carries this `ETag`,
    /// and `If-Range` with another tag gets the whole representation (`200`).
    pub etags: HashMap<i64, String>,
    /// Content connections open right now, and the most there ever were.
    pub open_content: usize,
    pub peak_content: usize,
    /// The JSON `GET /api/firmware` answers (404 when unset).
    pub firmware: Option<Value>,
}

pub struct FakeRomm {
    pub url: String,
    pub state: Arc<Mutex<State>>,
    pub release: Arc<Notify>,
}

impl FakeRomm {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let release = Arc::new(Notify::new());
        let (st, rel) = (state.clone(), release.clone());
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                tokio::spawn(serve(sock, st.clone(), rel.clone()));
            }
        });
        Self { url, state, release }
    }

    pub fn set_firmware(&self, list: Value) {
        self.state.lock().unwrap().firmware = Some(list);
    }

    pub fn set_rom(&self, rom: Value) {
        let id = rom["id"].as_i64().unwrap();
        self.state.lock().unwrap().roms.insert(id, rom);
    }

    pub fn set_content(&self, id: i64, bytes: Vec<u8>) {
        self.state.lock().unwrap().content.insert(id, bytes);
    }

    /// Serve `id` as a cached archive with this `ETag` (see [`State::etags`]).
    pub fn set_cached(&self, id: i64, bytes: Vec<u8>, etag: &str) {
        let mut st = self.state.lock().unwrap();
        st.content.insert(id, bytes);
        st.etags.insert(id, etag.to_string());
    }

    /// Content connections open right now.
    pub fn open_content_connections(&self) -> usize {
        self.state.lock().unwrap().open_content
    }

    /// The most content connections that were ever open at once.
    pub fn peak_content_connections(&self) -> usize {
        self.state.lock().unwrap().peak_content
    }

    pub fn stall_at(&self, at: Option<usize>) {
        self.state.lock().unwrap().stall_at = at;
    }

    pub fn content_requests(&self) -> Vec<Seen> {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.path.contains("/content/"))
            .cloned()
            .collect()
    }
}

async fn serve(mut sock: TcpStream, state: Arc<Mutex<State>>, release: Arc<Notify>) {
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
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let get = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    let (range, if_range) = (get("range"), get("if-range"));
    state.lock().unwrap().requests.push(Seen {
        path: path.clone(),
        range: range.clone(),
        if_range: if_range.clone(),
        accept_encoding: get("accept-encoding"),
    });
    if path == "/api/firmware" {
        let firmware = state.lock().unwrap().firmware.clone();
        let resp = match firmware {
            Some(list) => {
                let json = list.to_string();
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
                    json.len()
                )
            }
            None => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_string(),
        };
        let _ = sock.write_all(resp.as_bytes()).await;
        return;
    }
    let rest = path.strip_prefix("/api/roms/").unwrap_or("");
    let (id_part, content) = match rest.split_once("/content/") {
        Some((id, _)) => (id, true),
        None => (rest, false),
    };
    let id: Option<i64> = id_part.parse().ok();
    let (rom, body, stall, etag) = {
        let st = state.lock().unwrap();
        (
            id.and_then(|i| st.roms.get(&i).cloned()),
            id.and_then(|i| st.content.get(&i).cloned()),
            st.stall_at,
            id.and_then(|i| st.etags.get(&i).cloned()),
        )
    };
    if !content {
        let Some(rom) = rom else {
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
            return;
        };
        let json = rom.to_string();
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{json}",
            json.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        return;
    }
    let Some(body) = body else {
        let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        return;
    };
    let total = body.len();
    // A conditional range whose validator no longer matches gets the whole file.
    let stale = matches!((&if_range, &etag), (Some(given), Some(current)) if given != current);
    let start = range
        .as_deref()
        .filter(|_| !stale)
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
        .filter(|s| *s < total);
    // Validators accompany a ranged (cached) response only; a plain GET is the streamed ZIP.
    let validators = match (&etag, &range) {
        (Some(tag), Some(_)) => format!("etag: {tag}\r\nlast-modified: Mon, 01 Jan 2026 00:00:00 GMT\r\n"),
        _ => String::new(),
    };
    let head = match start {
        Some(s) => format!(
            "HTTP/1.1 206 Partial Content\r\naccept-ranges: bytes\r\ncontent-range: bytes {s}-{}/{total}\r\n\
             {validators}content-length: {}\r\nconnection: close\r\n\r\n",
            total - 1,
            total - s
        ),
        None => format!(
            "HTTP/1.1 200 OK\r\naccept-ranges: bytes\r\n{validators}content-length: {total}\r\nconnection: close\r\n\r\n"
        ),
    };
    {
        let mut st = state.lock().unwrap();
        st.open_content += 1;
        st.peak_content = st.peak_content.max(st.open_content);
    }
    serve_body(&mut sock, head, &body, start.unwrap_or(0), stall, &release).await;
    state.lock().unwrap().open_content -= 1;
}

async fn serve_body(
    sock: &mut TcpStream,
    head: String,
    body: &[u8],
    from: usize,
    stall: Option<usize>,
    release: &Notify,
) {
    let total = body.len();
    if sock.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    match stall.filter(|s| *s > from && *s < total) {
        Some(stop) => {
            if sock.write_all(&body[from..stop]).await.is_err() {
                return;
            }
            let _ = sock.flush().await;
            release.notified().await;
            let _ = sock.write_all(&body[stop..]).await;
        }
        None => {
            let _ = sock.write_all(&body[from..]).await;
        }
    }
    let _ = sock.flush().await;
}
