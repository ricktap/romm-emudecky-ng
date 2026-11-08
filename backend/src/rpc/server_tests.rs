use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use super::*;

struct Client {
    lines: tokio::io::Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
}

impl Client {
    async fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).await.unwrap();
        let (r, w) = stream.into_split();
        Self {
            lines: BufReader::new(r).lines(),
            write: w,
        }
    }
    async fn send_raw(&mut self, raw: &str) {
        self.write.write_all(raw.as_bytes()).await.unwrap();
    }
    async fn send(&mut self, v: Value) {
        self.send_raw(&protocol::encode(&v)).await;
    }
    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("timed out waiting for a frame")
            .unwrap()
            .expect("connection closed");
        serde_json::from_str(&line).unwrap()
    }
}

fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(Vec<Value>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Value, String>> + Send + 'static,
{
    Arc::new(move |args| Box::pin(f(args)))
}

struct Running {
    _dir: tempfile::TempDir,
    path: PathBuf,
    hub: Arc<EventHub>,
    server: Arc<Server>,
    done: tokio::task::JoinHandle<()>,
}

fn registry(hub: Arc<EventHub>) -> Registry {
    let mut r = Registry::new();
    r.insert(
        "add".into(),
        handler(|a| async move { Ok(json!(a[0].as_i64().unwrap() + a[1].as_i64().unwrap())) }),
    );
    r.insert(
        "slow".into(),
        handler(|_| async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(json!("slow"))
        }),
    );
    r.insert("boom".into(), handler(|_| async { Err("it broke".to_string()) }));
    r.insert(
        "emit_then_return".into(),
        handler(move |_| {
            let hub = hub.clone();
            async move {
                hub.emit("progress", vec![json!({"pct": 50})]);
                Ok(json!("done"))
            }
        }),
    );
    r
}

async fn start(hook: Option<ShutdownHook>, max_line: usize) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("backend.sock");
    let hub = Arc::new(EventHub::default());
    let server = Server::with_max_line(registry(hub.clone()), hub.clone(), hook, max_line);
    let listener = Server::bind(&path).unwrap();
    let done = tokio::spawn(server.clone().serve(listener, path.clone()));
    Running {
        _dir: dir,
        path,
        hub,
        server,
        done,
    }
}

#[tokio::test]
async fn list_methods_unknown_and_errors() {
    let rt = start(None, MAX_LINE_BYTES).await;
    let mut c = Client::connect(&rt.path).await;
    c.send(json!({"id": 1, "method": "__list_methods__"})).await;
    assert_eq!(
        c.recv().await,
        json!({"id": 1, "result": ["add", "boom", "emit_then_return", "slow"]})
    );
    c.send(json!({"id": 2, "method": "nope", "args": []})).await;
    assert_eq!(
        c.recv().await,
        json!({"id": 2, "error": {"message": "unknown method: nope"}})
    );
    c.send(json!({"id": 3, "method": "boom", "args": []})).await;
    assert_eq!(c.recv().await, json!({"id": 3, "error": {"message": "it broke"}}));
    c.send_raw("{not json\n").await;
    let bad = c.recv().await;
    assert_eq!(bad["id"], Value::Null);
    assert!(bad["error"]["message"].as_str().unwrap().starts_with("malformed frame"));
    c.send_raw("\n   \n").await; // blank lines are ignored
    c.send(json!({"id": 4, "method": "add", "args": [2, 5]})).await;
    assert_eq!(c.recv().await, json!({"id": 4, "result": 7}));
    assert_eq!(rt.server.method_names().len(), 4);
}

#[tokio::test]
async fn requests_run_concurrently() {
    let rt = start(None, MAX_LINE_BYTES).await;
    let mut c = Client::connect(&rt.path).await;
    c.send(json!({"id": 1, "method": "slow", "args": []})).await;
    c.send(json!({"id": 2, "method": "add", "args": [1, 1]})).await;
    assert_eq!(c.recv().await["id"], 2, "the fast call is not blocked by the slow one");
    assert_eq!(c.recv().await, json!({"id": 1, "result": "slow"}));
}

#[tokio::test]
async fn event_precedes_the_response_of_the_call_that_emitted_it() {
    let rt = start(None, MAX_LINE_BYTES).await;
    let mut c = Client::connect(&rt.path).await;
    c.send(json!({"id": 7, "method": "emit_then_return", "args": []})).await;
    assert_eq!(c.recv().await, json!({"event": "progress", "args": [{"pct": 50}]}));
    assert_eq!(c.recv().await, json!({"id": 7, "result": "done"}));
}

#[tokio::test]
async fn events_before_connect_are_buffered_and_bounded() {
    let rt = start(None, MAX_LINE_BYTES).await;
    for i in 0..(MAX_BUFFERED_EVENTS + 5) {
        rt.hub.emit("early", vec![json!(i)]);
    }
    let mut c = Client::connect(&rt.path).await;
    assert_eq!(c.recv().await, json!({"event": "early", "args": [0]}));
    for _ in 1..MAX_BUFFERED_EVENTS {
        c.recv().await;
    }
    c.send(json!({"id": 1, "method": "add", "args": [0, 0]})).await;
    assert_eq!(
        c.recv().await,
        json!({"id": 1, "result": 0}),
        "buffer capped; no extra events"
    );
    // A connected client receives live events directly.
    rt.hub.emit("live", vec![]);
    assert_eq!(c.recv().await, json!({"event": "live", "args": []}));
}

#[tokio::test]
async fn oversized_frame_is_rejected_and_connection_closed() {
    let rt = start(None, 64).await;
    let mut c = Client::connect(&rt.path).await;
    c.send_raw(&format!("{}\n", "x".repeat(200))).await;
    assert_eq!(
        c.recv().await,
        json!({"id": null, "error": {"message": "frame too large"}})
    );
    let closed = tokio::time::timeout(Duration::from_secs(5), c.lines.next_line())
        .await
        .unwrap()
        .unwrap();
    assert!(closed.is_none());
    // The server keeps serving other connections.
    let mut c2 = Client::connect(&rt.path).await;
    c2.send(json!({"id": 1, "method": "add", "args": [1, 2]})).await;
    assert_eq!(c2.recv().await["result"], 3);
}

#[tokio::test]
async fn shutdown_runs_hook_answers_and_closes() {
    let ran = Arc::new(AtomicUsize::new(0));
    let seen = ran.clone();
    let hook: ShutdownHook = Arc::new(move || {
        let seen = seen.clone();
        Box::pin(async move {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    });
    let rt = start(Some(hook), MAX_LINE_BYTES).await;
    let mut c = Client::connect(&rt.path).await;
    c.send(json!({"id": 1, "method": "slow", "args": []})).await;
    c.send(json!({"id": 2, "method": "__shutdown__"})).await;
    assert_eq!(c.recv().await, json!({"id": 2, "result": null}));
    tokio::time::timeout(Duration::from_secs(5), rt.done)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert!(!rt.path.exists(), "socket file removed");
    // The in-flight slow call was cancelled, not answered.
    let rest = tokio::time::timeout(Duration::from_secs(5), c.lines.next_line())
        .await
        .unwrap()
        .unwrap();
    assert!(rest.is_none());
}

#[tokio::test]
async fn shutdown_hook_error_is_reported_and_server_still_closes() {
    let hook: ShutdownHook = Arc::new(|| Box::pin(async { Err("flush failed".to_string()) }));
    let rt = start(Some(hook), MAX_LINE_BYTES).await;
    let mut c = Client::connect(&rt.path).await;
    c.send(json!({"id": 5, "method": "__shutdown__", "args": []})).await;
    assert_eq!(c.recv().await, json!({"id": 5, "error": {"message": "flush failed"}}));
    tokio::time::timeout(Duration::from_secs(5), rt.done)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn bind_replaces_stale_socket_and_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("backend.sock");
    std::fs::write(&path, b"stale").unwrap();
    let _listener = Server::bind(&path).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}

#[tokio::test]
async fn stop_without_hook_ends_serve() {
    let rt = start(None, MAX_LINE_BYTES).await;
    let _c = Client::connect(&rt.path).await;
    rt.server.stop();
    tokio::time::timeout(Duration::from_secs(5), rt.done)
        .await
        .unwrap()
        .unwrap();
}
