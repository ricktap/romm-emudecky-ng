//! Backend side of the protocol: serves a callable registry on a Unix socket.
//!
//! Each request runs in its own task, so a slow callable never blocks another.
//! Responses and events share one ordered outbound queue per connection, so an
//! event emitted while a call runs is written before that call's response.
//! Events emitted while no shim is connected are buffered for the first one.
//! The server knows nothing about what the callables do.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::net::unix::OwnedReadHalf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::rpc::protocol::{self, MAX_BUFFERED_EVENTS, MAX_LINE_BYTES};

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A callable: positional JSON args in, JSON result or error message out.
pub type Handler = Arc<dyn Fn(Vec<Value>) -> BoxFuture<Result<Value, String>> + Send + Sync>;

/// Callable name → handler.
pub type Registry = BTreeMap<String, Handler>;

/// Runs on `__shutdown__` before the server answers and closes.
pub type ShutdownHook = Arc<dyn Fn() -> BoxFuture<Result<(), String>> + Send + Sync>;

/// Fan-out of backend events to every connected shim.
#[derive(Default)]
pub struct EventHub {
    inner: Mutex<HubInner>,
}

#[derive(Default)]
struct HubInner {
    next_id: u64,
    clients: Vec<(u64, mpsc::UnboundedSender<String>)>,
    buffered: Vec<String>,
}

impl EventHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HubInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Send `{"event": name, "args": args}` to every client (or buffer it).
    pub fn emit(&self, name: &str, args: Vec<Value>) {
        let frame = protocol::encode(&protocol::event(name, args));
        let mut inner = self.lock();
        inner.clients.retain(|(_, tx)| !tx.is_closed());
        if inner.clients.is_empty() {
            if inner.buffered.len() < MAX_BUFFERED_EVENTS {
                inner.buffered.push(frame);
            }
            return;
        }
        for (_, tx) in &inner.clients {
            let _ = tx.send(frame.clone());
        }
    }

    /// Attach a client queue, flushing buffered events into it first.
    fn attach(&self, tx: mpsc::UnboundedSender<String>) -> u64 {
        let mut inner = self.lock();
        for frame in inner.buffered.drain(..) {
            let _ = tx.send(frame);
        }
        inner.next_id += 1;
        let id = inner.next_id;
        inner.clients.push((id, tx));
        id
    }

    fn detach(&self, id: u64) {
        self.lock().clients.retain(|(cid, _)| *cid != id);
    }
}

pub struct Server {
    registry: Registry,
    hub: Arc<EventHub>,
    on_shutdown: Option<ShutdownHook>,
    max_line: usize,
    stop: CancellationToken,
    tasks: TaskTracker,
}

enum Frame {
    Line,
    Eof,
    TooLarge,
}

/// Read one `\n`-terminated line into `buf` (without the newline).
async fn read_frame(reader: &mut BufReader<OwnedReadHalf>, buf: &mut Vec<u8>, max: usize) -> std::io::Result<Frame> {
    buf.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if buf.is_empty() { Frame::Eof } else { Frame::Line });
        }
        match available.iter().position(|b| *b == b'\n') {
            Some(i) => {
                buf.extend_from_slice(&available[..i]);
                reader.consume(i + 1);
                return Ok(if buf.len() > max { Frame::TooLarge } else { Frame::Line });
            }
            None => {
                let n = available.len();
                buf.extend_from_slice(available);
                reader.consume(n);
                if buf.len() > max {
                    return Ok(Frame::TooLarge);
                }
            }
        }
    }
}

impl Server {
    pub fn new(registry: Registry, hub: Arc<EventHub>, on_shutdown: Option<ShutdownHook>) -> Arc<Self> {
        Self::with_max_line(registry, hub, on_shutdown, MAX_LINE_BYTES)
    }

    pub fn with_max_line(
        registry: Registry,
        hub: Arc<EventHub>,
        on_shutdown: Option<ShutdownHook>,
        max_line: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            hub,
            on_shutdown,
            max_line,
            stop: CancellationToken::new(),
            tasks: TaskTracker::new(),
        })
    }

    /// The callable names this server answers, sorted.
    pub fn method_names(&self) -> Vec<String> {
        self.registry.keys().cloned().collect()
    }

    /// Ask the server to stop (as `__shutdown__` does, without the hook).
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Bind `socket_path` (replacing a stale socket file), owner-only.
    pub fn bind(socket_path: &Path) -> std::io::Result<UnixListener> {
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::remove_file(socket_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(socket_path)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// Accept and serve connections until stopped; then drain every connection,
    /// cancel in-flight calls, and remove the socket file.
    pub async fn serve(self: Arc<Self>, listener: UnixListener, socket_path: PathBuf) {
        loop {
            tokio::select! {
                _ = self.stop.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let me = self.clone();
                        self.tasks.spawn(async move { me.handle_connection(stream).await });
                    }
                    Err(e) => tracing::warn!("backend socket accept failed: {e}"),
                },
            }
        }
        drop(listener);
        self.tasks.close();
        self.tasks.wait().await;
        let _ = std::fs::remove_file(&socket_path);
    }

    async fn handle_connection(self: Arc<Self>, stream: tokio::net::UnixStream) {
        let (read_half, mut write_half) = stream.into_split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let client_id = self.hub.attach(tx.clone());
        let writer = tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                if write_half.write_all(frame.as_bytes()).await.is_err() {
                    break;
                }
            }
            let _ = write_half.shutdown().await;
        });

        let mut reader = BufReader::with_capacity(64 * 1024, read_half);
        let mut buf = Vec::new();
        let calls = TaskTracker::new();
        loop {
            let frame = tokio::select! {
                _ = self.stop.cancelled() => break,
                frame = read_frame(&mut reader, &mut buf, self.max_line) => frame,
            };
            match frame {
                Ok(Frame::Line) => {
                    if buf.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let line = std::mem::take(&mut buf);
                    let me = self.clone();
                    let tx = tx.clone();
                    calls.spawn(async move {
                        tokio::select! {
                            _ = me.stop.cancelled() => {}
                            _ = me.dispatch(&line, &tx) => {}
                        }
                    });
                }
                Ok(Frame::TooLarge) => {
                    let _ = tx.send(protocol::encode(&protocol::error_response(
                        &Value::Null,
                        "frame too large",
                    )));
                    break;
                }
                Ok(Frame::Eof) | Err(_) => break,
            }
        }
        self.hub.detach(client_id);
        calls.close();
        calls.wait().await;
        drop(tx);
        let _ = writer.await;
    }

    async fn dispatch(&self, line: &[u8], tx: &mpsc::UnboundedSender<String>) {
        let send = |message: Value| {
            let _ = tx.send(protocol::encode(&message));
        };
        let request = match protocol::decode_request(line) {
            Ok(r) => r,
            Err(bad) => return send(protocol::error_response(&bad.id, &bad.message)),
        };
        match request.method.as_str() {
            protocol::LIST_METHODS => send(protocol::result_response(&request.id, Value::from(self.method_names()))),
            protocol::SHUTDOWN => {
                let outcome = match &self.on_shutdown {
                    Some(hook) => hook().await,
                    None => Ok(()),
                };
                match outcome {
                    Ok(()) => send(protocol::result_response(&request.id, Value::Null)),
                    Err(msg) => {
                        tracing::error!("backend shutdown hook failed: {msg}");
                        send(protocol::error_response(&request.id, &msg));
                    }
                }
                self.stop.cancel();
            }
            name => match self.registry.get(name) {
                None => send(protocol::error_response(
                    &request.id,
                    &format!("unknown method: {name}"),
                )),
                Some(handler) => match handler(request.args).await {
                    Ok(result) => send(protocol::result_response(&request.id, result)),
                    Err(msg) => {
                        tracing::error!("backend callable '{name}' failed: {msg}");
                        send(protocol::error_response(&request.id, &msg));
                    }
                },
            },
        }
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
