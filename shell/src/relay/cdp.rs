//! CDP client: JSON-RPC over WebSocket — a port of the Go core's
//! cdp.go. Command calls wait up to 15 s; events go to a handler;
//! Notify is fire-and-forget but still consumes an id (like Go).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

pub type EventHandler = Arc<dyn Fn(String, Value) + Send + Sync>;

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;
type Sink = futures_util::stream::SplitSink<WsStream, Message>;
type Pending = Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>;

pub struct Cdp {
    tx: AsyncMutex<Sink>,
    pending: Pending,
    next: AtomicI64,
    closed: AtomicBool,
    read_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Cdp {
    pub async fn dial(
        ws_url: &str,
        origin: &str,
        on_event: EventHandler,
    ) -> Result<Arc<Cdp>, String> {
        let mut req = ws_url
            .into_client_request()
            .map_err(|e| format!("bad cdp url: {e}"))?;
        req.headers_mut().insert(
            "Origin",
            origin.parse().map_err(|_| "bad origin".to_string())?,
        );
        let (stream, _resp) = tokio_tungstenite::connect_async(req)
            .await
            .map_err(|e| format!("cdp connect: {e}"))?;
        let (sink, mut source) = stream.split();
        let cdp = Arc::new(Cdp {
            tx: AsyncMutex::new(sink),
            pending: Mutex::new(HashMap::new()),
            next: AtomicI64::new(0),
            closed: AtomicBool::new(false),
            read_task: Mutex::new(None),
        });
        let cdp2 = cdp.clone();
        let task = tokio::spawn(async move {
            while let Some(item) = source.next().await {
                let msg = match item {
                    Ok(m) => m,
                    Err(e) => {
                        cdp2.fail_all(&format!("cdp read: {e}"));
                        return;
                    }
                };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Close(_) => {
                        cdp2.fail_all("cdp connection closed");
                        return;
                    }
                    _ => continue,
                };
                let v: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(id) = v.get("id").and_then(|i| i.as_i64()) {
                    if let Some(ch) = cdp2.pending.lock().unwrap().remove(&id) {
                        let res = if let Some(err) = v.get("error") {
                            Err(err
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("cdp error")
                                .to_string())
                        } else {
                            Ok(v.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = ch.send(res);
                    }
                    continue;
                }
                if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
                    let params = v.get("params").cloned().unwrap_or(Value::Null);
                    on_event(method.to_string(), params);
                }
            }
            cdp2.fail_all("cdp connection closed");
        });
        *cdp.read_task.lock().unwrap() = Some(task);
        Ok(cdp)
    }

    fn fail_all(&self, err: &str) {
        self.closed.store(true, Ordering::SeqCst);
        let mut map = self.pending.lock().unwrap();
        for (_, ch) in map.drain() {
            let _ = ch.send(Err(err.to_string()));
        }
    }

    async fn write_text(&self, text: String) -> Result<(), String> {
        let mut sink = self.tx.lock().await;
        sink.send(Message::Text(text.into()))
            .await
            .map_err(|e| format!("cdp write: {e}"))
    }

    /// Send a command and wait for its response (15 s timeout).
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("cdp connection closed".into());
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let req = json!({"id": id, "method": method, "params": params});
        if let Err(e) = self.write_text(req.to_string()).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(Duration::from_secs(15), rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => Err("cdp connection closed".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("cdp call {method} timed out"))
            }
        }
    }

    /// Fire-and-forget command (screencast acks). Consumes an id.
    pub async fn notify(&self, method: &str, params: Value) {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let req = json!({"id": id, "method": method, "params": params});
        let _ = self.write_text(req.to_string()).await;
    }

    pub async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        {
            let mut sink = self.tx.lock().await;
            let _ = sink.close().await;
        }
        if let Some(task) = self.read_task.lock().unwrap().take() {
            task.abort();
        }
        self.fail_all("cdp connection closed");
    }
}
