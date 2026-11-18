use crate::cdp::client::CdpClient;
use crate::cdp::dialog::Dialog;
use crate::cdp::download::Download;
use crate::cdp::locator::Locator;
use crate::cdp::options::{
    CheckOptions, ClickOptions, CookieOptions, DblClickOptions, EmulateGeolocation,
    EmulateOptions, FilterOptions, GotoOptions, HoverOptions, InputFiles, KeyboardPressOptions,
    PageCloseOptions, ScreenshotOptions, SelectOptionOptions, SelectOptionValue,
    SetInputFilesOptions, ViewportOptions, WaitOptions,
};
use crate::cdp::protocol::{SerializedArgument, SerializedValue, SpecialValue};
use crate::cdp::route::{resolve_paused_request, RouteHandler, RouteRegistry};
use crate::error::Error;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_NAV_TIMEOUT: u64 = 30_000;

pub(crate) fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn storage_object(area: &str) -> Result<&'static str, Error> {
    match area {
        "local" | "localStorage" => Ok("window.localStorage"),
        "session" | "sessionStorage" => Ok("window.sessionStorage"),
        other => Err(Error::InvalidInput(format!(
            "unknown storage area: {} (expected 'local' or 'session')",
            other
        ))),
    }
}

pub(crate) fn element_expr(selector: &str, index: Option<usize>) -> String {
    format!(
        "(function(){{var sel={sel};var nodes;if(sel.indexOf('xpath=')===0){{var r=document.evaluate(sel.slice(6),document,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);nodes=[];for(var i=0;i<r.snapshotLength;i++){{nodes.push(r.snapshotItem(i));}}}}else{{nodes=Array.prototype.slice.call(document.querySelectorAll(sel));}}var el=nodes[{idx}]||null;return el;}})()",
        sel = js_str(selector),
        idx = index.unwrap_or(0)
    )
}

pub(crate) async fn evaluate_raw(client: &CdpClient, expression: &str) -> Result<Value, Error> {
    let params = json!({
        "expression": expression,
        "returnByValue": true,
        "awaitPromise": true,
    });
    let res = client.send("Runtime.evaluate", Some(params)).await?;
    if let Some(exc) = res.get("exceptionDetails") {
        let msg = exc
            .pointer("/exception/description")
            .and_then(|v| v.as_str())
            .unwrap_or("JavaScript exception");
        return Err(Error::ActionFailed(msg.to_string()));
    }
    Ok(res.get("result").cloned().unwrap_or(Value::Null))
}

pub(crate) async fn evaluate_value(client: &CdpClient, expression: &str) -> Result<Value, Error> {
    let result = evaluate_raw(client, expression).await?;
    Ok(result.get("value").cloned().unwrap_or(Value::Null))
}

pub(crate) async fn count_nodes(client: &CdpClient, selector: &str) -> Result<usize, Error> {
    let expr = format!(
        "(function(){{var sel={sel};if(sel.indexOf('xpath=')===0){{var r=document.evaluate(sel.slice(6),document,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);return r.snapshotLength;}}return document.querySelectorAll(sel).length;}})()",
        sel = js_str(selector)
    );
    let value = evaluate_value(client, &expr).await?;
    Ok(value.as_u64().unwrap_or(0) as usize)
}

pub(crate) async fn locator_visible(client: &CdpClient, selector: &str, index: Option<usize>) -> Result<bool, Error> {
    let expr = format!(
        "(function(){{var el={el};if(!el)return false;return !!(el.offsetWidth||el.offsetHeight||el.getClientRects().length);}})()",
        el = element_expr(selector, index)
    );
    let value = evaluate_value(client, &expr).await?;
    Ok(value.as_bool().unwrap_or(false))
}

pub(crate) async fn element_center(
    client: &CdpClient,
    selector: &str,
    index: Option<usize>,
) -> Result<(f64, f64), Error> {
    let expr = format!(
        "(function(){{var el={el};if(!el)return null;el.scrollIntoView({{block:'center',inline:'center'}});var r=el.getBoundingClientRect();return {{x:r.left+r.width/2,y:r.top+r.height/2}};}})()",
        el = element_expr(selector, index)
    );
    let result = evaluate_raw(client, &expr).await?;
    let value = result.get("value").cloned().unwrap_or(Value::Null);
    let x = value
        .get("x")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| Error::ActionFailed(format!("Element not found: {}", selector)))?;
    let y = value
        .get("y")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| Error::ActionFailed(format!("Element not found: {}", selector)))?;
    Ok((x, y))
}

pub(crate) async fn element_box(
    client: &CdpClient,
    selector: &str,
) -> Result<(f64, f64, f64, f64), Error> {
    let expr = format!(
        "(function(){{var el={el};if(!el)return null;var r=el.getBoundingClientRect();return {{x:r.left+window.scrollX,y:r.top+window.scrollY,width:r.width,height:r.height}};}})()",
        el = element_expr(selector, None)
    );
    let result = evaluate_raw(client, &expr).await?;
    let value = result.get("value").cloned().unwrap_or(Value::Null);
    let x = value.get("x").and_then(|v| v.as_f64());
    let y = value.get("y").and_then(|v| v.as_f64());
    let width = value.get("width").and_then(|v| v.as_f64());
    let height = value.get("height").and_then(|v| v.as_f64());
    match (x, y, width, height) {
        (Some(x), Some(y), Some(width), Some(height)) if width > 0.0 && height > 0.0 => {
            Ok((x, y, width, height))
        }
        _ => Err(Error::ActionFailed(format!("Element not found or has no box: {}", selector))),
    }
}

pub(crate) async fn dispatch_mouse(
    client: &CdpClient,
    type_: &str,
    x: f64,
    y: f64,
    button: &str,
    click_count: u32,
    buttons: i64,
) -> Result<(), Error> {
    let mut params = json!({
        "type": type_,
        "x": x,
        "y": y,
        "button": button,
        "clickCount": click_count,
    });
    if buttons > 0 {
        params["buttons"] = json!(buttons);
    }
    client.send("Input.dispatchMouseEvent", Some(params)).await?;
    Ok(())
}

fn remote_to_serialized(result: Option<&Value>) -> SerializedValue {
    let result = match result {
        Some(r) => r,
        None => return SerializedValue::default(),
    };
    let type_ = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if type_ == "undefined" {
        return SerializedValue::Special(SpecialValue::Undefined);
    }
    if let Some(value) = result.get("value") {
        return SerializedValue::from_json(value);
    }
    if result.get("subtype").and_then(|v| v.as_str()) == Some("null") {
        return SerializedValue::Special(SpecialValue::Null);
    }
    SerializedValue::Other(
        result
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    )
}

fn key_info(key: &str) -> (String, i64, Option<String>) {
    match key {
        "Enter" => ("Enter".to_string(), 13, Some("\r".to_string())),
        "Tab" => ("Tab".to_string(), 9, None),
        "Escape" | "Esc" => ("Escape".to_string(), 27, None),
        "Backspace" => ("Backspace".to_string(), 8, None),
        "Delete" => ("Delete".to_string(), 46, None),
        "ArrowLeft" | "Left" => ("ArrowLeft".to_string(), 37, None),
        "ArrowUp" | "Up" => ("ArrowUp".to_string(), 38, None),
        "ArrowRight" | "Right" => ("ArrowRight".to_string(), 39, None),
        "ArrowDown" | "Down" => ("ArrowDown".to_string(), 40, None),
        "Home" => ("Home".to_string(), 36, None),
        "End" => ("End".to_string(), 35, None),
        "PageUp" => ("PageUp".to_string(), 33, None),
        "PageDown" => ("PageDown".to_string(), 34, None),
        " " | "Space" => ("Space".to_string(), 32, Some(" ".to_string())),
        other => {
            let c = other.chars().next().unwrap_or(' ');
            let code = if c.is_ascii_alphabetic() {
                format!("Key{}", c.to_ascii_uppercase())
            } else if c.is_ascii_digit() {
                format!("Digit{}", c)
            } else {
                String::new()
            };
            let vk = c.to_ascii_uppercase() as i64;
            (code, vk, Some(other.to_string()))
        }
    }
}

fn mbps_to_bytes_per_second(mbps: f64) -> f64 {
    if mbps < 0.0 {
        -1.0
    } else {
        mbps * 1_000_000.0 / 8.0
    }
}

fn network_preset(preset: &str) -> Result<(bool, f64, f64, f64), Error> {
    match preset.trim().to_lowercase().replace('-', "_").as_str() {
        "offline" => Ok((true, 0.0, 0.0, 0.0)),
        "slow_3g" | "slow3g" => Ok((false, 400.0, 0.4, 0.4)),
        "fast_3g" | "fast3g" => Ok((false, 150.0, 1.6, 0.75)),
        "none" | "reset" => Ok((false, 0.0, -1.0, -1.0)),
        other => Err(Error::InvalidInput(format!(
            "unknown network preset: {} (expected offline, slow_3g, fast_3g, or none)",
            other
        ))),
    }
}

fn normalize_media(value: &str) -> Result<String, Error> {
    match value.trim().to_lowercase().as_str() {
        "screen" | "print" => Ok(value.trim().to_lowercase()),
        "none" | "reset" | "" => Ok(String::new()),
        other => Err(Error::InvalidInput(format!(
            "unknown media type: {} (expected screen, print, or none)",
            other
        ))),
    }
}

fn normalize_color_scheme(value: &str) -> Result<String, Error> {
    match value.trim().to_lowercase().as_str() {
        "light" | "dark" => Ok(value.trim().to_lowercase()),
        "none" | "reset" | "" => Ok(String::new()),
        other => Err(Error::InvalidInput(format!(
            "unknown color scheme: {} (expected light, dark, or none)",
            other
        ))),
    }
}

fn validate_geolocation(geo: &EmulateGeolocation) -> Result<(), Error> {
    if !(-90.0..=90.0).contains(&geo.latitude) {
        return Err(Error::InvalidInput("latitude must be between -90 and 90".to_string()));
    }
    if !(-180.0..=180.0).contains(&geo.longitude) {
        return Err(Error::InvalidInput("longitude must be between -180 and 180".to_string()));
    }
    if geo.accuracy < 0.0 {
        return Err(Error::InvalidInput("accuracy must not be negative".to_string()));
    }
    Ok(())
}

pub struct Page {
    client: Arc<CdpClient>,
    target_id: String,
    default_navigation_timeout: Mutex<u64>,
    url: Arc<Mutex<String>>,
    routes: Arc<RouteRegistry>,
}

impl Page {
    pub async fn new(client: Arc<CdpClient>) -> Result<Arc<Self>, Error> {
        let mut target_id = client.target_id().to_string();
        let mut url = String::new();

        if let Ok(info) = client.send("Target.getTargetInfo", None).await {
            url = info
                .pointer("/targetInfo/url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if target_id.is_empty() {
                if let Some(id) = info.pointer("/targetInfo/targetId").and_then(|v| v.as_str()) {
                    target_id = id.to_string();
                }
            }
        }

        let page = Arc::new(Page {
            client: client.clone(),
            target_id,
            default_navigation_timeout: Mutex::new(DEFAULT_NAV_TIMEOUT),
            url: Arc::new(Mutex::new(url)),
            routes: Arc::new(RouteRegistry::default()),
        });

        let _ = client.send("Page.enable", None).await;
        let _ = client.send("Runtime.enable", None).await;
        let _ = client.send("DOM.enable", None).await;
        let _ = client.send("Network.enable", None).await;

        page.setup_downloads().await;
        page.spawn_url_listener();
        page.spawn_fetch_listener();
        Ok(page)
    }

    async fn setup_downloads(&self) {
        let dir = self.client.download_dir().to_string_lossy().to_string();
        let params = json!({
            "behavior": "allow",
            "downloadPath": dir,
            "eventsEnabled": true,
        });
        let _ = self.client.send("Browser.setDownloadBehavior", Some(params.clone())).await;
        let _ = self.client.send("Page.setDownloadBehavior", Some(json!({
            "behavior": "allow",
            "downloadPath": dir,
        }))).await;
    }

    fn spawn_url_listener(self: &Arc<Self>) {
        let mut rx = self.client.subscribe();
        let cache = self.url.clone();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                match event.method.as_str() {
                    "Page.frameNavigated" => {
                        if let Some(frame) = event.params.get("frame") {
                            if frame.get("parentId").is_none() {
                                if let Some(url) = frame.get("url").and_then(|v| v.as_str()) {
                                    if let Ok(mut guard) = cache.lock() {
                                        *guard = url.to_string();
                                    }
                                }
                            }
                        }
                    }
                    "Page.navigatedWithinDocument" => {
                        if let Some(url) = event.params.get("url").and_then(|v| v.as_str()) {
                            if let Ok(mut guard) = cache.lock() {
                                *guard = url.to_string();
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    fn spawn_fetch_listener(self: &Arc<Self>) {
        let mut rx = self.client.subscribe();
        let routes = self.routes.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) if event.method == "Fetch.requestPaused" => {
                        let request_id = match event
                            .params
                            .get("requestId")
                            .and_then(|value| value.as_str())
                        {
                            Some(id) => id.to_string(),
                            None => continue,
                        };
                        let url = event
                            .params
                            .pointer("/request/url")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let handler = routes.match_handler(&url).await;
                        let client = client.clone();
                        tokio::spawn(async move {
                            resolve_paused_request(client, request_id, handler).await;
                        });
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        eprintln!("[CDP] Fetch listener lagged by {} events", count);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    pub async fn route(&self, pattern: &str, handler: RouteHandler) -> Result<(), Error> {
        let first = self.routes.add(pattern.to_string(), handler).await?;
        if first {
            if let Err(error) = self
                .client
                .send("Fetch.enable", Some(json!({ "patterns": [] })))
                .await
            {
                let _ = self.routes.remove(pattern).await;
                return Err(error);
            }
            self.routes.enable();
        }
        Ok(())
    }

    pub async fn unroute(&self, pattern: &str) -> Result<(), Error> {
        self.routes.remove(pattern).await;
        if self.routes.is_empty().await && self.routes.is_enabled() {
            self.client.send("Fetch.disable", None).await?;
            self.routes.disable();
        }
        Ok(())
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn client(&self) -> &Arc<CdpClient> {
        &self.client
    }

    async fn refresh_url(&self) {
        if let Ok(res) = self.client.send("Page.getFrameTree", None).await {
            if let Some(url) = res.pointer("/frameTree/frame/url").and_then(|v| v.as_str()) {
                if let Ok(mut guard) = self.url.lock() {
                    *guard = url.to_string();
                }
            }
        }
    }

    pub fn url(&self) -> String {
        self.url.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn set_default_navigation_timeout(&self, ms: u64) {
        if let Ok(mut guard) = self.default_navigation_timeout.lock() {
            *guard = ms;
        }
    }

    pub fn default_navigation_timeout(&self) -> u64 {
        self.default_navigation_timeout
            .lock()
            .map(|g| *g)
            .unwrap_or(DEFAULT_NAV_TIMEOUT)
    }

    pub async fn goto(&self, url: &str, opts: Option<GotoOptions>) -> Result<(), Error> {
        let wait_until = opts
            .as_ref()
            .and_then(|o| o.wait_until.clone())
            .unwrap_or_else(|| "load".to_string());
        let timeout = opts
            .as_ref()
            .and_then(|o| o.timeout)
            .unwrap_or_else(|| self.default_navigation_timeout());

        let res = self
            .client
            .send("Page.navigate", Some(json!({ "url": url })))
            .await?;

        if let Some(error_text) = res.get("errorText").and_then(|v| v.as_str()) {
            return Err(Error::ActionFailed(format!(
                "Failed to goto URL: {}",
                error_text
            )));
        }

        self.refresh_url().await;

        if wait_until != "commit" {
            self.wait_for_ready_state(&wait_until, timeout).await?;
            self.refresh_url().await;
        }

        Ok(())
    }

    async fn wait_for_ready_state(&self, wait_until: &str, timeout_ms: u64) -> Result<(), Error> {
        let start = Instant::now();
        loop {
            let state = evaluate_value(&self.client, "document.readyState")
                .await
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()));

            let done = match wait_until {
                "domcontentloaded" => matches!(state.as_deref(), Some("interactive") | Some("complete")),
                _ => state.as_deref() == Some("complete"),
            };

            if done {
                if wait_until == "networkidle" {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                return Ok(());
            }

            if start.elapsed() >= Duration::from_millis(timeout_ms) {
                return Err(Error::Timeout(format!("navigation ({})", wait_until)));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn reload(&self, ignore_cache: bool) -> Result<(), Error> {
        self.client
            .send("Page.reload", Some(json!({ "ignoreCache": ignore_cache })))
            .await?;
        self.refresh_url().await;
        Ok(())
    }

    pub async fn go_back(&self) -> Result<(), Error> {
        self.navigate_history(-1).await
    }

    pub async fn go_forward(&self) -> Result<(), Error> {
        self.navigate_history(1).await
    }

    async fn navigate_history(&self, delta: isize) -> Result<(), Error> {
        let history = self
            .client
            .send("Page.getNavigationHistory", None)
            .await?;
        let index = history
            .get("currentIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as isize;
        let entries = history
            .get("entries")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::ActionFailed("navigation history is unavailable".to_string()))?;
        let target = index + delta;
        if target < 0 || target >= entries.len() as isize {
            return Err(Error::ActionFailed(if delta < 0 {
                "no previous history entry".to_string()
            } else {
                "no next history entry".to_string()
            }));
        }
        let entry_id = entries[target as usize]
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::ActionFailed("history entry has no id".to_string()))?;
        self.client
            .send("Page.navigateToHistoryEntry", Some(json!({ "entryId": entry_id })))
            .await?;
        self.refresh_url().await;
        Ok(())
    }

    pub async fn screenshot(&self, opts: ScreenshotOptions) -> Result<(Vec<u8>, String), Error> {
        let format = opts.format.unwrap_or_else(|| "png".to_string());
        let mut params = json!({
            "format": format,
            "captureBeyondViewport": opts.full_page || opts.target.is_some(),
        });

        if let Some(target) = &opts.target {
            let (x, y, width, height) = element_box(&self.client, target).await?;
            params["clip"] = json!({
                "x": x,
                "y": y,
                "width": width,
                "height": height,
                "scale": 1,
            });
        } else if opts.full_page {
            let metrics = self.client.send("Page.getLayoutMetrics", None).await?;
            let width = metrics
                .pointer("/cssContentSize/width")
                .and_then(|v| v.as_f64())
                .or_else(|| metrics.pointer("/contentSize/width").and_then(|v| v.as_f64()))
                .unwrap_or(0.0);
            let height = metrics
                .pointer("/cssContentSize/height")
                .and_then(|v| v.as_f64())
                .or_else(|| metrics.pointer("/contentSize/height").and_then(|v| v.as_f64()))
                .unwrap_or(0.0);
            if width > 0.0 && height > 0.0 {
                params["clip"] = json!({
                    "x": 0,
                    "y": 0,
                    "width": width,
                    "height": height,
                    "scale": 1,
                });
            }
        }

        if format == "jpeg" {
            if let Some(quality) = opts.quality {
                params["quality"] = json!(quality);
            }
        }

        let result = self
            .client
            .send("Page.captureScreenshot", Some(params))
            .await?;
        let data = result
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::ActionFailed("screenshot returned no data".to_string()))?;
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
            .map_err(|e| Error::ActionFailed(format!("Invalid screenshot data: {}", e)))?;
        Ok((bytes, data.to_string()))
    }

    pub async fn set_viewport(&self, opts: ViewportOptions) -> Result<(), Error> {
        let params = json!({
            "width": opts.width,
            "height": opts.height,
            "deviceScaleFactor": opts.device_scale_factor.unwrap_or(1.0),
            "mobile": opts.mobile,
        });
        self.client
            .send("Emulation.setDeviceMetricsOverride", Some(params))
            .await?;
        Ok(())
    }

    pub async fn reset_viewport(&self) -> Result<(), Error> {
        self.client
            .send("Emulation.clearDeviceMetricsOverride", None)
            .await?;
        Ok(())
    }

    pub async fn emulate(&self, opts: EmulateOptions) -> Result<(), Error> {
        if opts.reset {
            self.client
                .send(
                    "Network.emulateNetworkConditions",
                    Some(json!({
                        "offline": false,
                        "latency": 0.0,
                        "downloadThroughput": -1.0,
                        "uploadThroughput": -1.0,
                    })),
                )
                .await?;
            self.client
                .send(
                    "Emulation.setEmulatedMedia",
                    Some(json!({ "media": "", "features": [] })),
                )
                .await?;
            self.client
                .send("Emulation.clearGeolocationOverride", None)
                .await?;
            self.client
                .send("Network.setUserAgentOverride", Some(json!({ "userAgent": "" })))
                .await?;
            self.client
                .send("Emulation.setTimezoneOverride", Some(json!({ "timezoneId": "" })))
                .await?;
        }

        let preset = opts.preset.as_deref().unwrap_or("");
        let wants_network = !preset.is_empty()
            || opts.offline.is_some()
            || opts.latency.is_some()
            || opts.download.is_some()
            || opts.upload.is_some();
        if wants_network {
            let (preset_offline, preset_latency, preset_download, preset_upload) = if preset.is_empty()
            {
                (false, 0.0, -1.0, -1.0)
            } else {
                network_preset(preset)?
            };
            let offline = opts.offline.unwrap_or(preset_offline);
            let latency = opts.latency.unwrap_or(preset_latency);
            let download = opts.download.unwrap_or(preset_download);
            let upload = opts.upload.unwrap_or(preset_upload);
            self.client
                .send(
                    "Network.emulateNetworkConditions",
                    Some(json!({
                        "offline": offline,
                        "latency": latency,
                        "downloadThroughput": mbps_to_bytes_per_second(download),
                        "uploadThroughput": mbps_to_bytes_per_second(upload),
                    })),
                )
                .await?;
        }

        let media = opts.media.as_deref().map(normalize_media).transpose()?;
        let color_scheme = opts
            .color_scheme
            .as_deref()
            .map(normalize_color_scheme)
            .transpose()?;
        if media.is_some() || color_scheme.is_some() {
            let features = match color_scheme.as_deref() {
                Some(value) if !value.is_empty() => vec![json!({
                    "name": "prefers-color-scheme",
                    "value": value,
                })],
                _ => vec![],
            };
            self.client
                .send(
                    "Emulation.setEmulatedMedia",
                    Some(json!({
                        "media": media.unwrap_or_default(),
                        "features": features,
                    })),
                )
                .await?;
        }

        if let Some(geo) = opts.geolocation {
            validate_geolocation(&geo)?;
            self.client
                .send(
                    "Emulation.setGeolocationOverride",
                    Some(json!({
                        "latitude": geo.latitude,
                        "longitude": geo.longitude,
                        "accuracy": geo.accuracy,
                    })),
                )
                .await?;
        }

        if let Some(user_agent) = opts.user_agent {
            self.client
                .send(
                    "Network.setUserAgentOverride",
                    Some(json!({ "userAgent": user_agent })),
                )
                .await?;
        }

        if let Some(timezone) = opts.timezone {
            self.client
                .send(
                    "Emulation.setTimezoneOverride",
                    Some(json!({ "timezoneId": timezone })),
                )
                .await?;
        }

        Ok(())
    }

    pub async fn get_cookies(&self, urls: Option<Vec<String>>) -> Result<Value, Error> {
        let params = match urls {
            Some(urls) => json!({ "urls": urls }),
            None => json!({}),
        };
        let result = self.client.send("Network.getCookies", Some(params)).await?;
        Ok(result.get("cookies").cloned().unwrap_or_else(|| json!([])))
    }

    pub async fn set_cookie(&self, opts: &CookieOptions) -> Result<(), Error> {
        let name = opts
            .name
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("cookies set requires 'name'".to_string()))?;
        let mut params = json!({ "name": name });
        if let Some(value) = &opts.value {
            params["value"] = json!(value);
        }
        if let Some(url) = &opts.url {
            params["url"] = json!(url);
        }
        if let Some(domain) = &opts.domain {
            params["domain"] = json!(domain);
        }
        if let Some(path) = &opts.path {
            params["path"] = json!(path);
        }
        self.client.send("Network.setCookie", Some(params)).await?;
        Ok(())
    }

    pub async fn delete_cookie(&self, opts: &CookieOptions) -> Result<(), Error> {
        let name = opts
            .name
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("cookies delete requires 'name'".to_string()))?;
        let mut params = json!({ "name": name });
        if let Some(url) = &opts.url {
            params["url"] = json!(url);
        }
        if let Some(domain) = &opts.domain {
            params["domain"] = json!(domain);
        }
        if let Some(path) = &opts.path {
            params["path"] = json!(path);
        }
        self.client.send("Network.deleteCookies", Some(params)).await?;
        Ok(())
    }

    pub async fn clear_cookies(&self) -> Result<(), Error> {
        self.client.send("Network.clearBrowserCookies", None).await?;
        Ok(())
    }

    pub async fn set_extra_headers(&self, headers: &HashMap<String, String>) -> Result<(), Error> {
        self.client
            .send("Network.setExtraHTTPHeaders", Some(json!({ "headers": headers })))
            .await?;
        Ok(())
    }

    pub async fn clear_extra_headers(&self) -> Result<(), Error> {
        self.client
            .send(
                "Network.setExtraHTTPHeaders",
                Some(json!({ "headers": {} })),
            )
            .await?;
        Ok(())
    }

    pub async fn storage_get(&self, area: &str, key: Option<&str>) -> Result<Value, Error> {
        let store = storage_object(area)?;
        let expr = match key {
            Some(key) => format!("{}.getItem({})", store, js_str(key)),
            None => format!(
                "(function(){{var s={};var o={{}};for(var i=0;i<s.length;i++){{var k=s.key(i);o[k]=s.getItem(k);}}return o;}})()",
                store
            ),
        };
        evaluate_value(&self.client, &expr).await
    }

    pub async fn storage_set(&self, area: &str, key: &str, value: &str) -> Result<(), Error> {
        let store = storage_object(area)?;
        let expr = format!(
            "(function(){{{}.setItem({},{});
            return true;}})()",
            store,
            js_str(key),
            js_str(value)
        );
        evaluate_value(&self.client, &expr).await?;
        Ok(())
    }

    pub async fn storage_delete(&self, area: &str, key: &str) -> Result<(), Error> {
        let store = storage_object(area)?;
        let expr = format!(
            "(function(){{{}.removeItem({});return true;}})()",
            store,
            js_str(key)
        );
        evaluate_value(&self.client, &expr).await?;
        Ok(())
    }

    pub async fn storage_clear(&self, area: &str) -> Result<(), Error> {
        let store = storage_object(area)?;
        let expr = format!("(function(){{{}.clear();return true;}})()", store);
        evaluate_value(&self.client, &expr).await?;
        Ok(())
    }

    pub async fn evaluate(
        &self,
        fn_source: &str,
        _arg: SerializedArgument,
        _is_function: Option<bool>,
    ) -> Result<SerializedValue, Error> {
        let params = json!({
            "expression": fn_source,
            "returnByValue": true,
            "awaitPromise": true,
        });
        let res = self.client.send("Runtime.evaluate", Some(params)).await?;
        if let Some(exc) = res.get("exceptionDetails") {
            let msg = exc
                .pointer("/exception/description")
                .and_then(|v| v.as_str())
                .unwrap_or("JavaScript exception");
            return Err(Error::ActionFailed(msg.to_string()));
        }
        Ok(remote_to_serialized(res.get("result")))
    }

    pub fn locator(&self, selector: &str, _options: Option<FilterOptions>) -> Locator {
        Locator::new(self.client.clone(), selector.to_string(), None)
    }

    pub async fn click(&self, selector: &str, opts: Option<ClickOptions>) -> Result<(), Error> {
        let (x, y) = element_center(&self.client, selector, None).await?;
        let button = opts
            .as_ref()
            .and_then(|o| o.button.clone())
            .unwrap_or_else(|| "left".to_string());
        let count = opts.as_ref().and_then(|o| o.click_count).unwrap_or(1);
        let buttons = match button.as_str() {
            "right" => 2,
            "middle" => 4,
            _ => 1,
        };
        dispatch_mouse(&self.client, "mouseMoved", x, y, "none", 0, 0).await?;
        dispatch_mouse(&self.client, "mousePressed", x, y, &button, count, buttons).await?;
        dispatch_mouse(&self.client, "mouseReleased", x, y, &button, count, buttons).await?;
        Ok(())
    }

    pub async fn dblclick(&self, selector: &str, _opts: Option<DblClickOptions>) -> Result<(), Error> {
        let (x, y) = element_center(&self.client, selector, None).await?;
        dispatch_mouse(&self.client, "mouseMoved", x, y, "none", 0, 0).await?;
        for count in 1..=2 {
            dispatch_mouse(&self.client, "mousePressed", x, y, "left", count, 1).await?;
            dispatch_mouse(&self.client, "mouseReleased", x, y, "left", count, 1).await?;
        }
        Ok(())
    }

    pub async fn hover(&self, selector: &str, _opts: Option<HoverOptions>) -> Result<(), Error> {
        let (x, y) = element_center(&self.client, selector, None).await?;
        dispatch_mouse(&self.client, "mouseMoved", x, y, "none", 0, 0).await?;
        Ok(())
    }

    pub async fn check(&self, selector: &str, _opts: Option<CheckOptions>) -> Result<(), Error> {
        self.set_checked(selector, true).await
    }

    pub async fn uncheck(&self, selector: &str, _opts: Option<CheckOptions>) -> Result<(), Error> {
        self.set_checked(selector, false).await
    }

    async fn set_checked(&self, selector: &str, checked: bool) -> Result<(), Error> {
        let expr = format!(
            "(function(){{var el={el};if(!el)return false;el.checked={checked};el.dispatchEvent(new Event('input',{{bubbles:true}}));el.dispatchEvent(new Event('change',{{bubbles:true}}));return true;}})()",
            el = element_expr(selector, None),
            checked = checked
        );
        evaluate_raw(&self.client, &expr).await?;
        Ok(())
    }

    pub async fn select_option(
        &self,
        selector: &str,
        values: Vec<SelectOptionValue>,
        _opts: Option<SelectOptionOptions>,
    ) -> Result<Vec<String>, Error> {
        let criteria: Vec<Value> = values
            .iter()
            .map(|v| {
                let mut map = serde_json::Map::new();
                if let Some(value) = &v.value {
                    map.insert("value".to_string(), json!(value));
                }
                if let Some(label) = &v.label {
                    map.insert("label".to_string(), json!(label));
                }
                if let Some(index) = v.index {
                    map.insert("index".to_string(), json!(index));
                }
                Value::Object(map)
            })
            .collect();

        let expr = format!(
            "(function(){{var el={el};if(!el)return [];var criteria={criteria};var out=[];var opts=Array.prototype.slice.call(el.options||[]);for(var i=0;i<opts.length;i++){{var o=opts[i];for(var j=0;j<criteria.length;j++){{var c=criteria[j];var match=(c.value!==undefined&&o.value===c.value)||(c.label!==undefined&&o.textContent===c.label)||(c.index!==undefined&&i===c.index);if(match){{o.selected=true;out.push(o.value);}}}}}}el.dispatchEvent(new Event('input',{{bubbles:true}}));el.dispatchEvent(new Event('change',{{bubbles:true}}));return out;}})()",
            el = element_expr(selector, None),
            criteria = serde_json::to_string(&criteria).unwrap_or_else(|_| "[]".to_string())
        );

        let value = evaluate_value(&self.client, &expr).await?;
        Ok(value
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn set_input_files(
        &self,
        selector: &str,
        files: InputFiles,
        _opts: Option<SetInputFilesOptions>,
    ) -> Result<(), Error> {
        let paths: Vec<String> = match files {
            InputFiles::Paths(paths) => paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            InputFiles::Payloads(_) => {
                return Err(Error::ActionFailed(
                    "in-memory file payloads are not supported".to_string(),
                ))
            }
        };

        let expr = format!(
            "(function(){{return {el};}})()",
            el = element_expr(selector, None)
        );
        let res = self
            .client
            .send(
                "Runtime.evaluate",
                Some(json!({ "expression": expr, "returnByValue": false })),
            )
            .await?;

        let object_id = res
            .pointer("/result/objectId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::ActionFailed(format!("Element not found: {}", selector)))?;

        let node = self
            .client
            .send("DOM.requestNode", Some(json!({ "objectId": object_id })))
            .await?;
        let node_id = node.get("nodeId").and_then(|v| v.as_u64()).unwrap_or(0);

        self.client
            .send(
                "DOM.setFileInputFiles",
                Some(json!({ "files": paths, "nodeId": node_id })),
            )
            .await?;
        Ok(())
    }

    pub async fn drag_and_drop(
        &self,
        source_selector: &str,
        target_selector: &str,
        _opts: Option<crate::cdp::options::DragAndDropOptions>,
    ) -> Result<(), Error> {
        let (sx, sy) = element_center(&self.client, source_selector, None).await?;
        let (tx, ty) = element_center(&self.client, target_selector, None).await?;

        dispatch_mouse(&self.client, "mouseMoved", sx, sy, "none", 0, 0).await?;
        dispatch_mouse(&self.client, "mousePressed", sx, sy, "left", 1, 1).await?;
        let steps = 5;
        for i in 1..=steps {
            let x = sx + (tx - sx) * (i as f64) / (steps as f64);
            let y = sy + (ty - sy) * (i as f64) / (steps as f64);
            dispatch_mouse(&self.client, "mouseMoved", x, y, "left", 1, 1).await?;
        }
        dispatch_mouse(&self.client, "mouseReleased", tx, ty, "left", 1, 1).await?;
        Ok(())
    }

    pub fn mouse(&self) -> Mouse {
        Mouse {
            client: self.client.clone(),
        }
    }

    pub fn keyboard(&self) -> Keyboard {
        Keyboard {
            client: self.client.clone(),
        }
    }

    pub async fn wait_for_selector(&self, selector: &str, opts: WaitOptions) -> Result<(), Error> {
        let timeout = opts.timeout.unwrap_or(2_000);
        let state = opts.state.unwrap_or_else(|| "visible".to_string());
        let start = Instant::now();

        loop {
            let count = count_nodes(&self.client, selector).await.unwrap_or(0);
            let ready = match state.as_str() {
                "attached" => count > 0,
                "hidden" => count == 0,
                "visible" => {
                    count > 0 && locator_visible(&self.client, selector, None).await.unwrap_or(false)
                }
                _ => count > 0,
            };

            if ready {
                return Ok(());
            }

            if start.elapsed() >= Duration::from_millis(timeout) {
                return Err(Error::Timeout(format!("Selector: {}", selector)));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn wait_for_dialog(&self, timeout_ms: u64) -> Result<Dialog, Error> {
        let mut rx = self.client.subscribe_dialogs();
        if self.client.take_dialog().await.is_some() {
            return Ok(Dialog::new(self.client.clone()));
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout("dialog".to_string()));
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(_)) => {
                    let _ = self.client.take_dialog().await;
                    return Ok(Dialog::new(self.client.clone()));
                }
                Ok(Err(_)) => {
                    return Err(Error::ActionFailed(
                        "dialog event channel closed".to_string(),
                    ))
                }
                Err(_) => return Err(Error::Timeout("dialog".to_string())),
            }
        }
    }

    pub async fn wait_for_download(&self, timeout_ms: u64) -> Result<Download, Error> {
        let mut rx = self.client.subscribe_downloads();
        if let Some(guid) = self.client.take_pending_download().await {
            return self.make_download(guid).await;
        }

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout("download".to_string()));
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(guid)) => return self.make_download(guid).await,
                Ok(Err(_)) => {
                    return Err(Error::ActionFailed(
                        "download event channel closed".to_string(),
                    ))
                }
                Err(_) => return Err(Error::Timeout("download".to_string())),
            }
        }
    }

    async fn make_download(&self, guid: String) -> Result<Download, Error> {
        let state = self
            .client
            .download_state(&guid)
            .await
            .ok_or_else(|| Error::ActionFailed("download not found".to_string()))?;
        Ok(Download::new(
            self.client.clone(),
            guid,
            state.suggested_filename,
        ))
    }

    pub async fn bring_to_front(&self) -> Result<(), Error> {
        self.client.send("Page.bringToFront", None).await?;
        Ok(())
    }

    pub async fn close(&self, _opts: Option<PageCloseOptions>) -> Result<(), Error> {
        self.client
            .send(
                "Target.closeTarget",
                Some(json!({ "targetId": self.target_id })),
            )
            .await?;
        Ok(())
    }
}

pub struct Mouse {
    client: Arc<CdpClient>,
}

impl Mouse {
    pub async fn wheel(&self, delta_x: f64, delta_y: f64) -> Result<(), Error> {
        let params = json!({
            "type": "mouseWheel",
            "x": 0,
            "y": 0,
            "deltaX": delta_x,
            "deltaY": delta_y,
        });
        self.client.send("Input.dispatchMouseEvent", Some(params)).await?;
        Ok(())
    }
}

pub struct Keyboard {
    client: Arc<CdpClient>,
}

impl Keyboard {
    pub async fn press(&self, key: &str, opts: Option<KeyboardPressOptions>) -> Result<(), Error> {
        let parts: Vec<&str> = key.split('+').collect();
        let mut modifiers = 0;
        let main_key = if parts.len() > 1 {
            for modifier in &parts[..parts.len() - 1] {
                modifiers |= match modifier.to_lowercase().as_str() {
                    "alt" => 1,
                    "control" | "ctrl" => 2,
                    "meta" | "command" | "cmd" => 4,
                    "shift" => 8,
                    _ => 0,
                };
            }
            parts[parts.len() - 1]
        } else {
            key
        };

        let (code, vk, text) = key_info(main_key);

        let mut down = json!({
            "type": "keyDown",
            "key": main_key,
            "windowsVirtualKeyCode": vk,
            "nativeVirtualKeyCode": vk,
            "modifiers": modifiers,
        });
        if !code.is_empty() {
            down["code"] = json!(code);
        }
        if let Some(text) = &text {
            down["text"] = json!(text);
        }
        self.client.send("Input.dispatchKeyEvent", Some(down)).await?;

        if let Some(delay) = opts.and_then(|o| o.delay) {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        let mut up = json!({
            "type": "keyUp",
            "key": main_key,
            "windowsVirtualKeyCode": vk,
            "nativeVirtualKeyCode": vk,
            "modifiers": modifiers,
        });
        if !code.is_empty() {
            up["code"] = json!(code);
        }
        self.client.send("Input.dispatchKeyEvent", Some(up)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    async fn mock_page() -> (Arc<Page>, mpsc::UnboundedReceiver<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                let text = match msg {
                    Message::Text(text) => text,
                    _ => continue,
                };
                let request: Value = match serde_json::from_str(&text) {
                    Ok(request) => request,
                    Err(_) => continue,
                };
                let id = match request.get("id") {
                    Some(id) => id.clone(),
                    None => continue,
                };
                let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let _ = tx.send(request.clone());
                let result = match method {
                    "Page.getLayoutMetrics" => {
                        json!({ "cssContentSize": { "width": 800.0, "height": 1600.0 } })
                    }
                    "Page.getNavigationHistory" => json!({
                        "currentIndex": 1,
                        "entries": [{ "id": 10 }, { "id": 20 }, { "id": 30 }],
                    }),
                    "Page.captureScreenshot" => json!({ "data": "aGVsbG8=" }),
                    "Network.getCookies" => {
                        json!({ "cookies": [{ "name": "session", "value": "abc" }] })
                    }
                    "Runtime.evaluate" => json!({
                        "result": {
                            "value": { "x": 10.0, "y": 20.0, "width": 100.0, "height": 50.0 }
                        }
                    }),
                    _ => json!({}),
                };
                let _ = ws
                    .send(Message::Text(json!({ "id": id, "result": result }).to_string()))
                    .await;
                if method == "Fetch.enable" {
                    let _ = ws
                        .send(Message::Text(
                            json!({
                                "method": "Fetch.requestPaused",
                                "params": {
                                    "requestId": "req-1",
                                    "request": { "url": "https://example.com/api/users" }
                                }
                            })
                            .to_string(),
                        ))
                        .await;
                }
            }
        });

        let client = CdpClient::connect(&format!("ws://{}", addr), "target".to_string())
            .await
            .unwrap();
        let page = Page::new(client).await.unwrap();
        (page, rx)
    }

    async fn take_requests(rx: &mut mpsc::UnboundedReceiver<Value>) -> Vec<Value> {
        let mut requests = Vec::new();
        while let Ok(Some(request)) =
            tokio::time::timeout(Duration::from_millis(20), rx.recv()).await
        {
            requests.push(request);
        }
        requests
    }

    fn find_request<'a>(requests: &'a [Value], method: &str) -> &'a Value {
        requests
            .iter()
            .find(|r| r.get("method").and_then(|m| m.as_str()) == Some(method))
            .unwrap_or_else(|| panic!("missing request: {}", method))
    }

    #[tokio::test]
    async fn screenshot_captures_full_page_and_decodes_data() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        let (bytes, data) = page
            .screenshot(ScreenshotOptions {
                format: Some("png".to_string()),
                quality: None,
                full_page: true,
                target: None,
            })
            .await
            .unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(data, "aGVsbG8=");

        let requests = take_requests(&mut rx).await;
        assert!(requests
            .iter()
            .any(|r| r.get("method").and_then(|m| m.as_str()) == Some("Page.getLayoutMetrics")));
        let capture = find_request(&requests, "Page.captureScreenshot");
        assert_eq!(capture.pointer("/params/format").and_then(|v| v.as_str()), Some("png"));
        assert_eq!(
            capture.pointer("/params/clip/height").and_then(|v| v.as_f64()),
            Some(1600.0)
        );
    }

    #[tokio::test]
    async fn history_navigation_uses_adjacent_entry_ids() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        page.go_back().await.unwrap();
        page.go_forward().await.unwrap();

        let requests = take_requests(&mut rx).await;
        let ids: Vec<i64> = requests
            .iter()
            .filter(|r| r.get("method").and_then(|m| m.as_str()) == Some("Page.navigateToHistoryEntry"))
            .filter_map(|r| r.pointer("/params/entryId").and_then(|v| v.as_i64()))
            .collect();
        assert_eq!(ids, vec![10, 30]);
    }

    #[tokio::test]
    async fn viewport_sets_and_clears_emulation() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        page.set_viewport(ViewportOptions {
            width: 1280,
            height: 720,
            device_scale_factor: Some(2.0),
            mobile: true,
        })
        .await
        .unwrap();
        page.reset_viewport().await.unwrap();

        let requests = take_requests(&mut rx).await;
        let set = find_request(&requests, "Emulation.setDeviceMetricsOverride");
        assert_eq!(set.pointer("/params/width").and_then(|v| v.as_u64()), Some(1280));
        assert_eq!(set.pointer("/params/height").and_then(|v| v.as_u64()), Some(720));
        assert_eq!(
            set.pointer("/params/deviceScaleFactor").and_then(|v| v.as_f64()),
            Some(2.0)
        );
        assert!(requests
            .iter()
            .any(|r| r.get("method").and_then(|m| m.as_str()) == Some("Emulation.clearDeviceMetricsOverride")));
    }

    #[tokio::test]
    async fn cookies_use_network_domain() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        let cookies = page
            .get_cookies(Some(vec!["https://example.com".to_string()]))
            .await
            .unwrap();
        assert_eq!(cookies[0]["name"], "session");
        page.set_cookie(&CookieOptions {
            name: Some("session".to_string()),
            value: Some("abc".to_string()),
            url: Some("https://example.com".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
        page.delete_cookie(&CookieOptions {
            name: Some("session".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();
        page.clear_cookies().await.unwrap();

        let requests = take_requests(&mut rx).await;
        assert_eq!(
            find_request(&requests, "Network.getCookies").pointer("/params/urls/0").and_then(|v| v.as_str()),
            Some("https://example.com")
        );
        assert_eq!(
            find_request(&requests, "Network.setCookie").pointer("/params/name").and_then(|v| v.as_str()),
            Some("session")
        );
        assert!(requests
            .iter()
            .any(|r| r.get("method").and_then(|m| m.as_str()) == Some("Network.deleteCookies")));
        assert!(requests
            .iter()
            .any(|r| r.get("method").and_then(|m| m.as_str()) == Some("Network.clearBrowserCookies")));
    }

    #[tokio::test]
    async fn headers_set_and_clear_extra_headers() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        let mut headers = HashMap::new();
        headers.insert("X-Key".to_string(), "value".to_string());
        page.set_extra_headers(&headers).await.unwrap();
        page.clear_extra_headers().await.unwrap();

        let requests = take_requests(&mut rx).await;
        assert_eq!(
            find_request(&requests, "Network.setExtraHTTPHeaders")
                .pointer("/params/headers/X-Key")
                .and_then(|v| v.as_str()),
            Some("value")
        );
        let clears: Vec<&Value> = requests
            .iter()
            .filter(|r| r.get("method").and_then(|m| m.as_str()) == Some("Network.setExtraHTTPHeaders"))
            .collect();
        assert_eq!(clears.len(), 2);
        assert_eq!(clears[1].pointer("/params/headers").and_then(|v| v.as_object()).map(|o| o.len()), Some(0));
    }

    #[tokio::test]
    async fn screenshot_target_uses_element_box_as_clip() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        let (bytes, _) = page
            .screenshot(ScreenshotOptions {
                format: None,
                quality: None,
                full_page: false,
                target: Some("#card".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(bytes, b"hello");

        let requests = take_requests(&mut rx).await;
        let capture = find_request(&requests, "Page.captureScreenshot");
        assert_eq!(capture.pointer("/params/clip/x").and_then(|v| v.as_f64()), Some(10.0));
        assert_eq!(capture.pointer("/params/clip/y").and_then(|v| v.as_f64()), Some(20.0));
        assert_eq!(capture.pointer("/params/clip/width").and_then(|v| v.as_f64()), Some(100.0));
        assert_eq!(capture.pointer("/params/clip/height").and_then(|v| v.as_f64()), Some(50.0));
        assert_eq!(
            capture.pointer("/params/captureBeyondViewport").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[tokio::test]
    async fn storage_uses_runtime_evaluate_for_both_areas() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        page.storage_set("local", "token", "abc").await.unwrap();
        let _ = page.storage_get("session", Some("token")).await.unwrap();
        page.storage_delete("local", "token").await.unwrap();
        page.storage_clear("session").await.unwrap();

        let requests = take_requests(&mut rx).await;
        let expressions: Vec<&str> = requests
            .iter()
            .filter(|r| r.get("method").and_then(|m| m.as_str()) == Some("Runtime.evaluate"))
            .filter_map(|r| r.pointer("/params/expression").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(expressions.len(), 4);
        assert!(expressions[0].contains("window.localStorage.setItem(\"token\",\"abc\")"));
        assert!(expressions[1].contains("window.sessionStorage.getItem(\"token\")"));
        assert!(expressions[2].contains("window.localStorage.removeItem(\"token\")"));
        assert!(expressions[3].contains("window.sessionStorage.clear()"));
    }

    #[tokio::test]
    async fn emulate_sends_cdp_overrides() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        page.emulate(EmulateOptions {
            preset: Some("slow_3g".to_string()),
            color_scheme: Some("dark".to_string()),
            geolocation: Some(EmulateGeolocation {
                latitude: 37.7749,
                longitude: -122.4194,
                accuracy: 1.0,
            }),
            user_agent: Some("test-agent".to_string()),
            timezone: Some("UTC".to_string()),
            ..Default::default()
        })
        .await
        .unwrap();

        let requests = take_requests(&mut rx).await;
        let network = find_request(&requests, "Network.emulateNetworkConditions");
        assert_eq!(network.pointer("/params/offline").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(network.pointer("/params/latency").and_then(|v| v.as_f64()), Some(400.0));
        assert_eq!(
            network.pointer("/params/downloadThroughput").and_then(|v| v.as_f64()),
            Some(50_000.0)
        );
        assert_eq!(
            network.pointer("/params/uploadThroughput").and_then(|v| v.as_f64()),
            Some(50_000.0)
        );

        let media = find_request(&requests, "Emulation.setEmulatedMedia");
        assert_eq!(media.pointer("/params/features/0/name").and_then(|v| v.as_str()), Some("prefers-color-scheme"));
        assert_eq!(media.pointer("/params/features/0/value").and_then(|v| v.as_str()), Some("dark"));

        let geo = find_request(&requests, "Emulation.setGeolocationOverride");
        assert_eq!(geo.pointer("/params/latitude").and_then(|v| v.as_f64()), Some(37.7749));
        assert_eq!(geo.pointer("/params/longitude").and_then(|v| v.as_f64()), Some(-122.4194));

        assert_eq!(
            find_request(&requests, "Network.setUserAgentOverride").pointer("/params/userAgent").and_then(|v| v.as_str()),
            Some("test-agent")
        );
        assert_eq!(
            find_request(&requests, "Emulation.setTimezoneOverride").pointer("/params/timezoneId").and_then(|v| v.as_str()),
            Some("UTC")
        );
    }

    #[tokio::test]
    async fn emulate_reset_sends_clear_commands() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        page.emulate(EmulateOptions {
            reset: true,
            ..Default::default()
        })
        .await
        .unwrap();

        let requests = take_requests(&mut rx).await;
        for method in [
            "Network.emulateNetworkConditions",
            "Emulation.setEmulatedMedia",
            "Emulation.clearGeolocationOverride",
            "Network.setUserAgentOverride",
            "Emulation.setTimezoneOverride",
        ] {
            assert!(requests.iter().any(|r| r.get("method").and_then(|m| m.as_str()) == Some(method)), "missing {}", method);
        }
    }

    #[tokio::test]
    async fn route_fulfills_matching_request() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        page.route(
            "**/api/**",
            RouteHandler::Fulfill {
                status: 201,
                headers: vec![("Content-Type".to_string(), "application/json".to_string())],
                body: r#"{"ok":true}"#.to_string(),
            },
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let requests = take_requests(&mut rx).await;
        let fulfill = find_request(&requests, "Fetch.fulfillRequest");
        assert_eq!(fulfill.pointer("/params/requestId").and_then(|v| v.as_str()), Some("req-1"));
        assert_eq!(fulfill.pointer("/params/responseCode").and_then(|v| v.as_u64()), Some(201));
        assert_eq!(
            fulfill.pointer("/params/responseHeaders/0/name").and_then(|v| v.as_str()),
            Some("Content-Type")
        );
        assert_eq!(
            fulfill.pointer("/params/body").and_then(|v| v.as_str()),
            Some("eyJvayI6dHJ1ZX0=")
        );
    }

    #[tokio::test]
    async fn routes_enable_and_disable_fetch_once() {
        let (page, mut rx) = mock_page().await;
        let _ = take_requests(&mut rx).await;

        page.route("**/api/**", RouteHandler::Continue).await.unwrap();
        page.route(
            "**/mock/**",
            RouteHandler::Fulfill {
                status: 200,
                headers: vec![],
                body: "ok".to_string(),
            },
        )
        .await
        .unwrap();
        page.unroute("**/api/**").await.unwrap();
        page.unroute("**/mock/**").await.unwrap();

        let requests = take_requests(&mut rx).await;
        assert_eq!(
            requests.iter().filter(|r| r.get("method").and_then(|m| m.as_str()) == Some("Fetch.enable")).count(),
            1
        );
        assert_eq!(
            requests.iter().filter(|r| r.get("method").and_then(|m| m.as_str()) == Some("Fetch.disable")).count(),
            1
        );
    }
}
