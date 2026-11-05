use crate::cdp::client::CdpClient;
use crate::cdp::dialog::Dialog;
use crate::cdp::download::Download;
use crate::cdp::locator::Locator;
use crate::cdp::options::{
    CheckOptions, ClickOptions, DblClickOptions, FilterOptions, GotoOptions, HoverOptions,
    InputFiles, KeyboardPressOptions, PageCloseOptions, SelectOptionOptions, SelectOptionValue,
    SetInputFilesOptions, WaitOptions,
};
use crate::cdp::protocol::{SerializedArgument, SerializedValue, SpecialValue};
use crate::error::Error;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_NAV_TIMEOUT: u64 = 30_000;

pub(crate) fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
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

pub struct Page {
    client: Arc<CdpClient>,
    target_id: String,
    default_navigation_timeout: Mutex<u64>,
    url: Arc<Mutex<String>>,
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
        });

        let _ = client.send("Page.enable", None).await;
        let _ = client.send("Runtime.enable", None).await;
        let _ = client.send("DOM.enable", None).await;

        page.setup_downloads().await;
        page.spawn_url_listener();
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

    pub async fn reload(&self, _opts: Option<GotoOptions>) -> Result<(), Error> {
        self.client
            .send("Page.reload", Some(json!({ "ignoreCache": false })))
            .await?;
        self.refresh_url().await;
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
