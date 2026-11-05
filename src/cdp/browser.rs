use crate::cdp::client::CdpClient;
use crate::cdp::page::Page;
use crate::error::Error;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

struct PageCache {
    order: Vec<String>,
    pages: HashMap<String, Arc<Page>>,
}

impl PageCache {
    fn new() -> Self {
        PageCache {
            order: Vec::new(),
            pages: HashMap::new(),
        }
    }

    fn insert(&mut self, target_id: String, page: Arc<Page>) {
        if !self.order.contains(&target_id) {
            self.order.push(target_id.clone());
        }
        self.pages.insert(target_id, page);
    }
}

struct BrowserInner {
    origin: String,
    initial_client: Arc<CdpClient>,
    initial_page: Arc<Page>,
    pages: Mutex<PageCache>,
}

pub struct Browser {
    inner: Arc<BrowserInner>,
}

impl Browser {
    pub async fn page(&self) -> Result<Arc<Page>, Error> {
        Ok(self.inner.initial_page.clone())
    }

    pub fn default_context(&self) -> Context {
        Context {
            inner: self.inner.clone(),
        }
    }

    pub async fn new_page(&self) -> Result<Arc<Page>, Error> {
        let res = self
            .inner
            .initial_client
            .send(
                "Target.createTarget",
                Some(json!({ "url": "about:blank" })),
            )
            .await?;
        let target_id = res
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::ActionFailed("Target.createTarget did not return a targetId".to_string())
            })?
            .to_string();
        self.inner.page_for_target(&target_id).await
    }
}

impl BrowserInner {
    async fn page_for_target(&self, target_id: &str) -> Result<Arc<Page>, Error> {
        {
            let cache = self.pages.lock().await;
            if let Some(page) = cache.pages.get(target_id) {
                return Ok(page.clone());
            }
        }

        let ws_url = format!("{}/devtools/page/{}", self.origin, target_id);
        let client = CdpClient::connect(&ws_url, target_id.to_string()).await?;
        let page = Page::new(client).await?;

        let mut cache = self.pages.lock().await;
        cache.insert(target_id.to_string(), page.clone());
        Ok(page)
    }
}

pub struct Context {
    inner: Arc<BrowserInner>,
}

impl Context {
    pub async fn pages(&self) -> Result<Vec<Arc<Page>>, Error> {
        let res = self
            .inner
            .initial_client
            .send("Target.getTargets", Some(json!({})))
            .await?;

        let target_ids: Vec<String> = res
            .get("targetInfos")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter(|t| {
                        t.get("type").and_then(|v| v.as_str()) == Some("page")
                    })
                    .filter_map(|t| t.get("targetId").and_then(|v| v.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let mut result = Vec::new();

        // First, add existing known pages in insertion order
        {
            let cache = self.inner.pages.lock().await;
            for id in &cache.order {
                if target_ids.iter().any(|x| x == id) {
                    if let Some(page) = cache.pages.get(id) {
                        result.push(page.clone());
                    }
                }
            }
        }

        // Then discover any new pages not yet cached
        for id in &target_ids {
            if !result.iter().any(|p| p.target_id() == id) {
                if let Ok(page) = self.inner.page_for_target(id).await {
                    result.push(page);
                }
            }
        }

        Ok(result)
    }
}

pub async fn connect(endpoint: &str) -> Result<Browser, Error> {
    let (ws_url, origin, target_id) = resolve_endpoint(endpoint).await?;

    let client = CdpClient::connect(&ws_url, target_id).await?;
    let page = Page::new(client.clone()).await?;

    let mut cache = PageCache::new();
    cache.insert(page.target_id().to_string(), page.clone());

    let inner = Arc::new(BrowserInner {
        origin,
        initial_client: client,
        initial_page: page,
        pages: Mutex::new(cache),
    });

    Ok(Browser { inner })
}

async fn resolve_endpoint(endpoint: &str) -> Result<(String, String, String), Error> {
    if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") {
        let origin = ws_origin(endpoint);
        let target_id = parse_target_id(endpoint).unwrap_or_default();
        return Ok((endpoint.to_string(), origin, target_id));
    }

    let base = endpoint.trim_end_matches('/');
    let list_url = format!("{}/json/list", base);
    let response = reqwest::get(&list_url)
        .await
        .map_err(|e| Error::ConnectionFailed(format!("Failed to query {}: {}", list_url, e)))?;

    if !response.status().is_success() {
        return Err(Error::ConnectionFailed(format!(
            "Failed to query {}: HTTP {}",
            list_url,
            response.status()
        )));
    }

    let targets: Value = response
        .json()
        .await
        .map_err(|e| Error::ConnectionFailed(format!("Failed to parse target list: {}", e)))?;

    let targets = targets
        .as_array()
        .ok_or_else(|| Error::ConnectionFailed("Target list is not an array".to_string()))?;

    let target = targets
        .iter()
        .find(|t| {
            t.get("type").and_then(|v| v.as_str()) == Some("page")
                && t.get("webSocketDebuggerUrl").and_then(|v| v.as_str()).is_some()
        })
        .or_else(|| {
            targets
                .iter()
                .find(|t| t.get("webSocketDebuggerUrl").and_then(|v| v.as_str()).is_some())
        })
        .ok_or_else(|| {
            Error::ConnectionFailed(format!("No debuggable target found at {}", list_url))
        })?;

    let ws_url = target
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::ConnectionFailed("Target has no webSocketDebuggerUrl".to_string()))?
        .to_string();

    let target_id = target
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| parse_target_id(&ws_url))
        .unwrap_or_default();

    let origin = ws_origin(&ws_url);
    Ok((ws_url, origin, target_id))
}

fn ws_origin(url: &str) -> String {
    let scheme_end = url.find("://").map(|i| i + 3).unwrap_or(0);
    let rest = &url[scheme_end..];
    match rest.find('/') {
        Some(pos) => url[..scheme_end + pos].to_string(),
        None => url.to_string(),
    }
}

fn parse_target_id(url: &str) -> Option<String> {
    let marker = "/devtools/page/";
    let pos = url.find(marker)? + marker.len();
    let rest = &url[pos..];
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    if end == 0 {
        None
    } else {
        Some(rest[..end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    async fn spawn_ws() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                let text = match msg {
                    tokio_tungstenite::tungstenite::Message::Text(text) => text,
                    _ => continue,
                };
                let value: Value = match serde_json::from_str(&text) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let id = value.get("id").cloned().unwrap_or(Value::Null);
                let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let expression = value
                    .pointer("/params/expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let result = match method {
                    "Target.getTargetInfo" => json!({
                        "targetInfo": { "targetId": "target-1", "url": "about:blank" }
                    }),
                    "Page.getFrameTree" => json!({
                        "frameTree": { "frame": { "url": "https://example.com/" } }
                    }),
                    "Runtime.evaluate" => {
                        let value = if expression.contains("querySelectorAll") {
                            json!(2)
                        } else if expression.contains("readyState") {
                            json!("complete")
                        } else {
                            json!("ok")
                        };
                        json!({ "result": { "type": "string", "value": value } })
                    }
                    _ => json!({}),
                };

                let response = json!({ "id": id, "result": result });
                let _ = ws.send(tokio_tungstenite::tungstenite::Message::Text(response.to_string())).await;
            }
        });
        addr
    }

    async fn spawn_http(ws_addr: std::net::SocketAddr) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let body = json!([{
                "type": "page",
                "id": "target-1",
                "webSocketDebuggerUrl": format!("ws://{}/devtools/page/target-1", ws_addr),
            }])
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        addr
    }

    #[tokio::test]
    async fn discovers_target_and_runs_page_helpers() {
        let ws_addr = spawn_ws().await;
        let http_addr = spawn_http(ws_addr).await;

        let browser = connect(&format!("http://{}", http_addr)).await.unwrap();
        let page = browser.page().await.unwrap();

        assert_eq!(page.target_id(), "target-1");

        page.goto("https://example.com/", None).await.unwrap();
        assert_eq!(page.url(), "https://example.com/");

        assert_eq!(page.locator("h1", None).count().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn resolves_explicit_page_websocket() {
        let ws_addr = spawn_ws().await;
        let browser = connect(&format!(
            "ws://{}/devtools/page/target-1",
            ws_addr
        ))
        .await
        .unwrap();
        let page = browser.page().await.unwrap();
        assert_eq!(page.target_id(), "target-1");
    }
}
