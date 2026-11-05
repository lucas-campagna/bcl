use crate::error::Error;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type WsRead = futures_util::stream::SplitStream<WsStream>;

#[derive(Debug, Clone)]
pub struct CdpError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone)]
pub struct DownloadState {
    pub guid: String,
    pub url: String,
    pub suggested_filename: String,
    pub state: String,
    pub received_bytes: f64,
    pub total_bytes: f64,
}

struct Inner {
    writer: Mutex<Option<WsSink>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, CdpError>>>>,
    events: broadcast::Sender<CdpEvent>,
    dialogs: Mutex<VecDeque<Value>>,
    dialogs_tx: broadcast::Sender<Value>,
    downloads: Mutex<HashMap<String, DownloadState>>,
    download_queue: Mutex<VecDeque<String>>,
    downloads_tx: broadcast::Sender<String>,
    download_dir: PathBuf,
}

pub struct CdpClient {
    inner: Arc<Inner>,
    next_id: AtomicU64,
    target_id: String,
}

impl CdpClient {
    pub async fn connect(ws_url: &str, target_id: String) -> Result<Arc<Self>, Error> {
        let (ws, _resp) = connect_async(ws_url).await.map_err(|e| {
            Error::ConnectionFailed(format!("WebSocket connect to {} failed: {}", ws_url, e))
        })?;
        Self::from_ws(ws, target_id).await
    }

    async fn from_ws(ws: WsStream, target_id: String) -> Result<Arc<Self>, Error> {
        let (sink, stream) = ws.split();

        let download_dir = std::env::var("DOWNLOAD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/downloads"));
        let _ = std::fs::create_dir_all(&download_dir);

        let (events, _) = broadcast::channel(1024);
        let (dialogs_tx, _) = broadcast::channel(64);
        let (downloads_tx, _) = broadcast::channel(64);

        let inner = Arc::new(Inner {
            writer: Mutex::new(Some(sink)),
            pending: Mutex::new(HashMap::new()),
            events,
            dialogs: Mutex::new(VecDeque::new()),
            dialogs_tx,
            downloads: Mutex::new(HashMap::new()),
            download_queue: Mutex::new(VecDeque::new()),
            downloads_tx,
            download_dir,
        });

        let client = Arc::new(CdpClient {
            inner: inner.clone(),
            next_id: AtomicU64::new(1),
            target_id,
        });

        tokio::spawn(reader_loop(stream, inner));
        Ok(client)
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn download_dir(&self) -> &PathBuf {
        &self.inner.download_dir
    }

    pub async fn send(&self, method: &str, params: Option<Value>) -> Result<Value, Error> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, tx);

        let mut obj = serde_json::Map::new();
        obj.insert("id".to_string(), json!(id));
        obj.insert("method".to_string(), json!(method));
        if let Some(params) = params {
            obj.insert("params".to_string(), params);
        }
        let text = Value::Object(obj).to_string();

        let send_result = {
            let mut writer = self.inner.writer.lock().await;
            match writer.as_mut() {
                Some(sink) => sink.send(Message::Text(text)).await,
                None => {
                    self.inner.pending.lock().await.remove(&id);
                    return Err(Error::ConnectionFailed("CDP connection closed".to_string()));
                }
            }
        };
        if let Err(e) = send_result {
            self.inner.pending.lock().await.remove(&id);
            return Err(Error::ConnectionFailed(format!(
                "Failed to send CDP command {}: {}",
                method, e
            )));
        }

        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(map_cdp_error(method, e)),
            Err(_) => Err(Error::ConnectionFailed(
                "CDP connection dropped while waiting for response".to_string(),
            )),
        }
    }

    pub async fn close(&self) {
        let mut writer = self.inner.writer.lock().await;
        if let Some(mut sink) = writer.take() {
            let _ = sink.close().await;
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.inner.events.subscribe()
    }

    pub async fn take_dialog(&self) -> Option<Value> {
        self.inner.dialogs.lock().await.pop_front()
    }

    pub fn subscribe_dialogs(&self) -> broadcast::Receiver<Value> {
        self.inner.dialogs_tx.subscribe()
    }

    pub async fn take_pending_download(&self) -> Option<String> {
        self.inner.download_queue.lock().await.pop_front()
    }

    pub async fn download_state(&self, guid: &str) -> Option<DownloadState> {
        self.inner.downloads.lock().await.get(guid).cloned()
    }

    pub fn subscribe_downloads(&self) -> broadcast::Receiver<String> {
        self.inner.downloads_tx.subscribe()
    }
}

async fn reader_loop(mut stream: WsRead, inner: Arc<Inner>) {
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => handle_text(&inner, &text).await,
            Some(Ok(Message::Binary(bytes))) => {
                if let Ok(text) = String::from_utf8(bytes) {
                    handle_text(&inner, &text).await;
                }
            }
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(_)) => {}
            Some(Err(_)) => break,
        }
    }

    let mut pending = inner.pending.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(CdpError {
            code: -1,
            message: "CDP connection closed".to_string(),
        }));
    }
}

async fn handle_text(inner: &Arc<Inner>, text: &str) {
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return,
    };

    if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
        let tx = inner.pending.lock().await.remove(&id);
        if let Some(tx) = tx {
            if let Some(err) = value.get("error") {
                let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
                let message = err
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("CDP error")
                    .to_string();
                let _ = tx.send(Err(CdpError { code, message }));
            } else {
                let result = value.get("result").cloned().unwrap_or(Value::Null);
                let _ = tx.send(Ok(result));
            }
        }
        return;
    }

    let method = match value.get("method").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => return,
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);

    match method.as_str() {
        "Page.downloadWillBegin" | "Browser.downloadWillBegin" => {
            let guid = params
                .get("guid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !guid.is_empty() {
                let state = DownloadState {
                    guid: guid.clone(),
                    url: params
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    suggested_filename: params
                        .get("suggestedFilename")
                        .and_then(|v| v.as_str())
                        .unwrap_or("download")
                        .to_string(),
                    state: "inProgress".to_string(),
                    received_bytes: 0.0,
                    total_bytes: 0.0,
                };
                inner.downloads.lock().await.insert(guid.clone(), state);
                inner.download_queue.lock().await.push_back(guid.clone());
                let _ = inner.downloads_tx.send(guid);
            }
        }
        "Page.downloadProgress" | "Browser.downloadProgress" => {
            let guid = params
                .get("guid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut downloads = inner.downloads.lock().await;
            if let Some(state) = downloads.get_mut(&guid) {
                if let Some(v) = params.get("receivedBytes").and_then(|v| v.as_f64()) {
                    state.received_bytes = v;
                }
                if let Some(v) = params.get("totalBytes").and_then(|v| v.as_f64()) {
                    state.total_bytes = v;
                }
                if let Some(v) = params.get("state").and_then(|v| v.as_str()) {
                    state.state = v.to_string();
                }
            }
        }
        "Page.javascriptDialogOpening" => {
            inner.dialogs.lock().await.push_back(params.clone());
            let _ = inner.dialogs_tx.send(params.clone());
        }
        _ => {}
    }

    let _ = inner.events.send(CdpEvent { method, params });
}

fn map_cdp_error(method: &str, error: CdpError) -> Error {
    match error.code {
        -32601 => Error::ActionFailed(format!("{}: method not found", method)),
        -32602 => Error::ActionFailed(format!("{}: invalid params ({})", method, error.message)),
        _ => Error::ActionFailed(format!("{}: {}", method, error.message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    async fn spawn_mock() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                let text = match msg {
                    Message::Text(text) => text,
                    _ => continue,
                };
                let value: Value = match serde_json::from_str(&text) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let id = value.get("id").cloned().unwrap_or(Value::Null);
                let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                match method {
                    "Page.enable" => {
                        let event = json!({
                            "method": "Page.frameNavigated",
                            "params": { "frame": { "url": "https://example.com" } }
                        });
                        let _ = ws.send(Message::Text(event.to_string())).await;
                    }
                    "Trigger.dialog" => {
                        let event = json!({
                            "method": "Page.javascriptDialogOpening",
                            "params": { "type": "alert", "message": "hi" }
                        });
                        let _ = ws.send(Message::Text(event.to_string())).await;
                    }
                    "Fail" => {
                        let response = json!({
                            "id": id,
                            "error": { "code": -32601, "message": "nope" }
                        });
                        let _ = ws.send(Message::Text(response.to_string())).await;
                        continue;
                    }
                    _ => {}
                }
                let response = json!({ "id": id, "result": { "ok": true } });
                let _ = ws.send(Message::Text(response.to_string())).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn sends_commands_and_dispatches_events() {
        let addr = spawn_mock().await;
        let client = CdpClient::connect(&format!("ws://{}", addr), "target".to_string())
            .await
            .unwrap();
        let mut events = client.subscribe();

        let result = client.send("Page.enable", None).await.unwrap();
        assert_eq!(result["ok"], true);

        let event = events.recv().await.unwrap();
        assert_eq!(event.method, "Page.frameNavigated");
        assert_eq!(
            event.params.pointer("/frame/url").and_then(|v| v.as_str()),
            Some("https://example.com")
        );
    }

    #[tokio::test]
    async fn queues_dialogs_and_maps_errors() {
        let addr = spawn_mock().await;
        let client = CdpClient::connect(&format!("ws://{}", addr), "target".to_string())
            .await
            .unwrap();

        client.send("Trigger.dialog", None).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let dialog = client.take_dialog().await.unwrap();
        assert_eq!(dialog["message"], "hi");

        let err = client.send("Fail", None).await.unwrap_err();
        assert!(matches!(err, Error::ActionFailed(_)));
        assert!(err.to_string().contains("method not found"));
    }
}
