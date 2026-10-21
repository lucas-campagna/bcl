use crate::error::Error;
use crate::flow::{Action, Condition, Context, FlowFile, Selector, SelectorPath, TabTarget};
use crate::executor::selector;
use crate::executor::utils::{get_timeout, parse_builtin_variables, ensure_page_initialized};
use ferridriver::options::{InputFiles, SelectOptionOptions, SelectOptionValue, SetInputFilesOptions};
use reqwest::Client;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn is_timeout_error(err: &Error) -> bool {
    matches!(err, Error::Timeout(_))
}

async fn handle_action_error(
    ws_url: &str,
    err: Error,
    on_error: &Option<Vec<Action>>,
    on_timeout: &Option<Vec<Action>>,
    context: &mut Context,
    verbose: bool,
    flows: &FlowFile,
) -> Result<(), Error> {
    let has_handler = is_timeout_error(&err) && on_timeout.is_some()
        || on_error.is_some();

    if !has_handler {
        if verbose {
            eprintln!("[EXEC] action failed: {}", err);
        }
        return Err(err);
    }

    let handler = if is_timeout_error(&err) {
        on_timeout.as_ref().or(on_error.as_ref())
    } else {
        on_error.as_ref()
    };

    if let Some(handler_actions) = handler {
        Box::pin(run_actions(ws_url, handler_actions, context, verbose, flows)).await?;
    }

    Ok(())
}

async fn resolve_page_for_tab(
    browser: &ferridriver::Browser,
    tab_target: TabTarget,
    verbose: bool,
) -> Result<Arc<ferridriver::Page>, Error> {
    match tab_target {
        TabTarget::New => {
            if verbose {
                eprintln!("[EXEC] Creating new tab");
            }
            browser
                .new_page()
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to create new tab: {}", e)))
        }
        TabTarget::Existing(index) => {
            if verbose {
                eprintln!("[EXEC] Switching to tab {}", index);
            }
            let context = browser.default_context();
            let pages = context
                .pages()
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to get pages: {}", e)))?;
            if index == 0 || index > pages.len() {
                return Err(Error::ActionFailed(format!(
                    "Tab index {} out of range (1-{}), no such tab",
                    index,
                    pages.len()
                )));
            }
            let target_page = pages.get(index - 1).unwrap().clone();
            target_page
                .bring_to_front()
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to bring tab to front: {}", e)))?;
            Ok(target_page)
        }
    }
}

pub async fn run_actions(
    ws_url: &str,
    actions: &[Action],
    context: &mut Context,
    verbose: bool,
    flows: &FlowFile,
) -> Result<(), Error> {
    let browser = if ws_url.starts_with("ws://") || ws_url.starts_with("wss://") {
        if verbose {
            eprintln!("[CONN] Using WebSocket connect: {}", ws_url);
        }
        ferridriver::chromium()
            .connect(ws_url, ferridriver::options::ConnectOptions::default())
            .await
    } else {
        if verbose {
            eprintln!("[CONN] Using CDP connect_over_cdp: {}", ws_url);
        }
        ferridriver::chromium()
            .connect_over_cdp(
                ws_url,
                ferridriver::options::ConnectOverCdpOptions::default(),
            )
            .await
    }
    .map_err(|e| Error::ConnectionFailed(format!("Failed to connect to browser: {}", e)))?;

    let browser = Arc::new(browser);

    let page = browser
        .page()
        .await
        .map_err(|e| Error::ConnectionFailed(format!("Failed to get page: {}", e)))?;

    let page: Rc<RefCell<Arc<ferridriver::Page>>> = Rc::new(RefCell::new(page));

    page.borrow().set_default_navigation_timeout(5000);

    for action in actions {
        let start = Instant::now();
        match action {
            Action::Goto { url, wait_until, tab } => {
                if let Some(tab_target) = tab {
                    let new_page = resolve_page_for_tab(&browser, *tab_target, verbose).await?;
                    new_page.set_default_navigation_timeout(5000);
                    *page.borrow_mut() = new_page;
                }
                let current_page = page.borrow();
                let resolved_url = context.resolve_with_url(url, Some(&current_page.url()));
                if verbose {
                    eprintln!("[EXEC] goto: url={}", resolved_url);
                }
                let wait_until = wait_until.clone().unwrap_or_else(|| "commit".to_string());
                current_page
                    .goto(
                        &resolved_url,
                        Some(ferridriver::options::GotoOptions {
                            wait_until: Some(wait_until),
                            ..Default::default()
                        }),
                    )
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to goto URL: {}", e)))?;
            }
            Action::Tab { index, close } => {
                if verbose {
                    eprintln!("[EXEC] tab: index={}, close={}", index, close);
                }
                let context_ref = browser.default_context();
                let pages = context_ref
                    .pages()
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to get pages: {}", e)))?;
                if *index == 0 || *index > pages.len() {
                    return Err(Error::ActionFailed(format!(
                        "Tab index {} out of range (1-{}), no such tab",
                        index,
                        pages.len()
                    )));
                }
                if *close {
                    let target_page = pages.get(*index - 1).unwrap().clone();
                    target_page
                        .close(None)
                        .await
                        .map_err(|e| Error::ActionFailed(format!("Failed to close tab: {}", e)))?;
                } else {
                    let target_page = pages.get(*index - 1).unwrap().clone();
                    target_page
                        .bring_to_front()
                        .await
                        .map_err(|e| Error::ActionFailed(format!("Failed to bring tab to front: {}", e)))?;
                    *page.borrow_mut() = target_page;
                }
            }
            Action::Type {
                value,
                target,
                secret,
                timeout: action_timeout,
                on_error,
                on_timeout,
                shadow_root,
                iframe,
            } => {
                let current_page = page.borrow();
                let result = do_type(&*current_page, context, value, target, *secret, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Click { target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let current_page = page.borrow();
                let result = do_click(&*current_page, target, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Press { key } => {
                let current_page = page.borrow();
                let resolved_key = context.resolve_with_url(key, Some(&current_page.url()));
                if verbose {
                    eprintln!("[EXEC] press: key={}", resolved_key);
                }
                current_page.keyboard()
                    .press(&resolved_key, None)
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to press key: {}", e)))?;
            }
            Action::Wait {
                url,
                target,
                duration,
                timeout: action_timeout,
                on_error,
                on_timeout,
                shadow_root,
                iframe,
            } => {
                let current_page = page.borrow();
                let result = do_wait(&*current_page, context, url, target, duration, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Ask { prompt, to } => {
                if verbose {
                    eprintln!("[EXEC] ask: prompt={}, to={}", prompt, to);
                }
                let value = ask_user(prompt)?;
                context.set(to, &value);
            }
            Action::Hover { target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let current_page = page.borrow();
                let result = do_hover(&*current_page, target, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::If {
                condition,
                then,
                else_,
            } => {
                let current_page = page.borrow();
                let condition_met = evaluate_condition(&*current_page, condition, context, verbose).await?;
                drop(current_page);
                if verbose {
                    eprintln!("[EXEC] if: condition={}", condition_met);
                }
                let branch = if condition_met { then } else { else_ };
                Box::pin(run_actions(ws_url, branch, context, verbose, flows)).await?;
            }
            Action::Read {
                target,
                to,
                html,
                attribute,
                value,
                timeout: action_timeout,
                on_error,
                on_timeout,
                shadow_root,
                iframe,
            } => {
                let current_page = page.borrow();
                let result = do_read(&*current_page, context, target, to, *html, attribute.as_deref(), *value, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Select { value, target, values, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe } => {
                let current_page = page.borrow();
                let result = do_select(&*current_page, context, value, target, values, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Check { target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let current_page = page.borrow();
                let result = do_check(&*current_page, target, *action_timeout, shadow_root, iframe, true, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Uncheck { target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let current_page = page.borrow();
                let result = do_check(&*current_page, target, *action_timeout, shadow_root, iframe, false, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::DblClick { target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let current_page = page.borrow();
                let result = do_dblclick(&*current_page, target, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::RightClick { target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let current_page = page.borrow();
                let result = do_right_click(&*current_page, target, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Clear { target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let current_page = page.borrow();
                let result = do_clear(&*current_page, target, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Upload { files, target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe } => {
                let current_page = page.borrow();
                let result = do_upload(&*current_page, files, target, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Drag { source, to, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let current_page = page.borrow();
                let result = do_drag(&*current_page, context, source, to, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Scroll { target, x, y, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe } => {
                let current_page = page.borrow();
                let result = do_scroll(&*current_page, context, target, *x, *y, *action_timeout, shadow_root, iframe, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Dialog { mode, prompt_text, timeout } => {
                if verbose {
                    eprintln!("[EXEC] dialog: mode={}, timeout={:?}", mode, timeout);
                }
                let page_clone = Arc::clone(&*page.borrow());
                let mode_str = mode.clone();
                let prompt = prompt_text.clone();
                let timeout_ms = timeout.unwrap_or(5000);
                tokio::spawn(async move {
                    match page_clone.wait_for_dialog(timeout_ms).await {
                        Ok(dialog) => {
                            match mode_str.as_str() {
                                "accept" => {
                                    dialog.accept(prompt.clone()).await.ok();
                                }
                                "dismiss" => {
                                    dialog.dismiss().await.ok();
                                }
                                _ => {}
                            }
                        }
                        Err(_) => {}
                    }
                });
            }
            Action::Download { path, timeout, to } => {
                if verbose {
                    eprintln!("[EXEC] download: path={}, timeout={:?}, to={:?}", path, timeout, to);
                }
                let timeout_ms = timeout.unwrap_or(30000);
                let current_page = page.borrow();
                let download = current_page.wait_for_download(timeout_ms)
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to wait for download: {}", e)))?;
                drop(current_page);
                let target_path = std::path::Path::new(&path);
                download.save_as(target_path)
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to save download: {}", e)))?;
                if let Some(var_name) = to {
                    context.set(var_name, path);
                }
            }
            Action::Log { text } => {
                let current_page = page.borrow();
                let resolved_text = parse_builtin_variables(&*current_page, text).await?;
                drop(current_page);
                let resolved_text = context.resolve(&resolved_text);
                if verbose {
                    eprintln!("[EXEC] log: text={}", resolved_text);
                }
                println!("{}", resolved_text);
            }
            Action::Call { flow, params } => {
                if verbose {
                    eprintln!("[EXEC] call: flow={}, params={:?}", flow, params);
                }
                let sub_actions = flows
                    .get(flow)
                    .ok_or_else(|| Error::ActionFailed(format!("Flow '{}' not found", flow)))?;
                let mut sub_context = context.clone();
                for (key, value) in params {
                    let value_str = serde_yaml::to_string(value).unwrap_or_default();
                    sub_context.set(key, value_str.trim());
                }
                Box::pin(run_actions(
                    ws_url,
                    sub_actions,
                    &mut sub_context,
                    verbose,
                    flows,
                ))
                .await?;
            }
            Action::Js { code, to, on_error } => {
                let current_page = page.borrow();
                let result = do_js(&*current_page, context, code, to, verbose).await;
                drop(current_page);
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, &None, context, verbose, flows).await?;
                }
            }
            Action::Define { var, value, overwrite } => {
                let current_page = page.borrow();
                if *overwrite || !context.is_defined(var) {
                    let resolved_value = resolve_yaml_value(value, context, Some(&current_page.url()));
                    if verbose {
                        eprintln!("[EXEC] define: {}={:?}", var, resolved_value);
                    }
                    context.set_yaml(var, resolved_value);
                } else if verbose {
                    eprintln!("[EXEC] define: {} (skipped, already defined)", var);
                }
            }
            Action::For { items, as_, do_ } => {
                if verbose {
                    eprintln!("[EXEC] for: items={:?}, as={}", items, as_);
                }
                let item_values: Vec<serde_yaml::Value> = match items {
                    serde_yaml::Value::Sequence(seq) => seq.clone(),
                    serde_yaml::Value::Number(n) => {
                        let count = n.as_u64().unwrap_or(0);
                        (0..count).map(|i| serde_yaml::Value::Number(i.into())).collect()
                    }
                    serde_yaml::Value::String(s) => {
                        if let Ok(v) = s.parse::<u64>() {
                            (0..v).map(|i| serde_yaml::Value::Number(i.into())).collect()
                        } else {
                            vec![serde_yaml::Value::String(s.clone())]
                        }
                    }
                    _ => Vec::new(),
                };
                for item in item_values {
                    let mut sub_context = context.clone();
                    match &item {
                        serde_yaml::Value::String(s) => sub_context.set(as_, s),
                        serde_yaml::Value::Number(n) => sub_context.set(as_, &n.to_string()),
                        _ => {
                            let yaml_str = serde_yaml::to_string(&item).unwrap_or_default();
                            sub_context.set(as_, yaml_str.trim());
                        }
                    }
                    Box::pin(run_actions(ws_url, do_, &mut sub_context, verbose, flows)).await?;
                }
            }
            Action::While { condition, do_, max } => {
                if verbose {
                    eprintln!("[EXEC] while: max={:?}", max);
                }
                let max_iter = max.unwrap_or(100);
                let mut iterations = 0;
                loop {
                    let current_page = page.borrow();
                    iterations += 1;
                    if iterations > max_iter {
                        if verbose {
                            eprintln!("[EXEC] while: max iterations ({}) reached", max_iter);
                        }
                        break;
                    }
                    let cond_met = evaluate_condition(&*current_page, condition, context, verbose).await?;
                    drop(current_page);
                    if !cond_met {
                        break;
                    }
                    Box::pin(run_actions(ws_url, do_, context, verbose, flows)).await?;
                }
            }
            Action::Retry { attempts, interval, do_ } => {
                if verbose {
                    eprintln!("[EXEC] retry: attempts={}, interval={:?}", attempts, interval);
                }
                let mut last_err = None;
                for attempt in 1..=*attempts {
                    match Box::pin(run_actions(ws_url, do_, context, verbose, flows)).await {
                        Ok(_) => {
                            last_err = None;
                            break;
                        }
                        Err(e) => {
                            let err_msg = e.to_string();
                            last_err = Some(err_msg.clone());
                            if verbose {
                                eprintln!("[EXEC] retry: attempt {}/{} failed: {}", attempt, attempts, e);
                            }
                            if attempt < *attempts {
                                if let Some(interval_ms) = interval {
                                    tokio::time::sleep(Duration::from_millis(*interval_ms)).await;
                                }
                            }
                        }
                    }
                }
                if let Some(err_msg) = last_err {
                    return Err(Error::ActionFailed(err_msg));
                }
            }
            Action::Assert { condition, message } => {
                if verbose {
                    eprintln!("[EXEC] assert");
                }
                let current_page = page.borrow();
                let cond_met = evaluate_condition(&*current_page, condition, context, verbose).await?;
                drop(current_page);
                if !cond_met {
                    let msg = message.as_ref().map(|s| s.as_str()).unwrap_or("assertion failed");
                    return Err(Error::Assertion(msg.to_string()));
                }
            }
            Action::Return { values } => {
                if verbose {
                    eprintln!("[EXEC] return: {:?}", values);
                }
                let current_page = page.borrow();
                for (key, value) in values {
                    let resolved = resolve_yaml_value(value, context, Some(&current_page.url()));
                    context.set_yaml(&format!("__return.{}", key), resolved);
                }
                drop(current_page);
            }
            Action::Fail { message } => {
                return Err(Error::ActionFailed(message.clone()));
            }
            Action::Save { path } => {
                if verbose {
                    eprintln!("[EXEC] save: path={}", path);
                }
                let vars = context.vars().clone();
                let json = serde_json::to_string_pretty(&vars)
                    .map_err(|e| Error::ActionFailed(format!("Failed to serialize context: {}", e)))?;
                std::fs::write(path, json)
                    .map_err(|e| Error::ActionFailed(format!("Failed to write save file: {}", e)))?;
            }
            Action::Load { path } => {
                if verbose {
                    eprintln!("[EXEC] load: path={}", path);
                }
                let content = std::fs::read_to_string(path)
                    .map_err(|e| Error::ActionFailed(format!("Failed to read load file: {}", e)))?;
                let vars: std::collections::HashMap<String, serde_json::Value> = serde_json::from_str(&content)
                    .map_err(|e| Error::ActionFailed(format!("Failed to parse save file: {}", e)))?;
                for (k, v) in vars {
                    let yaml_value: serde_yaml::Value = serde_yaml::from_value(serde_yaml::Value::String(v.as_str().unwrap_or_default().to_string()))
                        .unwrap_or_else(|_| serde_yaml::Value::String(v.as_str().unwrap_or_default().to_string()));
                    context.set_yaml(&k, yaml_value);
                }
            }
            Action::Request { url, method, headers, body, to, timeout, on_error } => {
                let result = do_request(context, url, method, headers, body, to, *timeout, verbose).await;
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, &None, context, verbose, flows).await?;
                }
            }
            Action::Server { base_path, port, ws_url, routes } => {
                let flows_clone = flows.clone();
                let routes_clone = routes.clone();
                let ws_url_clone = ws_url.clone();
                let verbose = verbose;
                run_server(base_path.clone(), *port, ws_url_clone, routes_clone, flows_clone, verbose).await?;
            }
        }
        if verbose {
            eprintln!("[EXEC] done: {:?}", start.elapsed());
        }
    }

    Ok(())
}

fn selector_to_string(selector: &Selector) -> String {
    match selector {
        Selector::XPath(x) => format!("xpath={}", x),
        Selector::Css(c) => c.clone(),
    }
}

async fn do_type(
    page: &Arc<ferridriver::Page>,
    context: &mut Context,
    value: &str,
    target: &Option<String>,
    secret: bool,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let resolved_value = context.resolve_value_with_url(value, Some(&page.url()))?;
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            let display_value = if secret {
                "****".to_string()
            } else {
                resolved_value.clone()
            };
            eprintln!(
                "[EXEC] type: value={}, selector={}, shadow_root={:?}, iframe={:?}",
                display_value, selector_str, shadow_root, iframe
            );
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            let count = page
                .locator(&selector_str, None)
                .count()
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to count elements: {}", e)))?;
            if count > 1 {
                let chars: Vec<char> = resolved_value.chars().collect();
                let locators = page.locator(&selector_str, None).all().await.map_err(|e| {
                    Error::ActionFailed(format!("Failed to get all locators: {}", e))
                })?;
                for (i, locator) in locators.into_iter().enumerate() {
                    if i < chars.len() {
                        let char_str = chars[i].to_string();
                        locator.fill(&char_str, None).await.map_err(|e| {
                            Error::ActionFailed(format!("Failed to type char '{}': {}", char_str, e))
                        })?;
                    }
                }
            } else {
                page.locator(&selector_str, None)
                    .first()
                    .fill(&resolved_value, None)
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to type: {}", e)))?;
            }
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "type", &resolved_value);
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to type via shadow DOM: {}", e)))?;
        }
    }
    Ok(())
}

fn build_shadow_dom_js_for_action(
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    final_selector: &str,
    action: &str,
    value: &str,
) -> String {
    let mut js = String::from("(function() { ");

    if let Some(iframe_path) = iframe {
        let selectors: Vec<String> = match iframe_path {
            SelectorPath::Single(s) => vec![s.clone()],
            SelectorPath::Multiple(v) => v.clone(),
        };
        for (i, sel) in selectors.iter().enumerate() {
            if i == 0 {
                js.push_str(&format!("var node = document.querySelector('{}'); ", sel));
                js.push_str("if (node && node.tagName === 'IFRAME') { node = node.contentDocument || node.contentWindow.document; } ");
                js.push_str("else if (node && node.shadowRoot) { node = node.shadowRoot; } ");
            } else {
                js.push_str(&format!("node = node.querySelector('{}'); ", sel));
                js.push_str("if (node && node.shadowRoot) { node = node.shadowRoot; } ");
            }
            js.push_str("if (!node) return null; ");
        }
    }

    if let Some(shadow_path) = shadow_root {
        let selectors: Vec<String> = match shadow_path {
            SelectorPath::Single(s) => vec![s.clone()],
            SelectorPath::Multiple(v) => v.clone(),
        };
        for sel in &selectors {
            js.push_str(&format!("node = node.querySelector('{}'); ", sel));
            js.push_str("if (node && node.shadowRoot) { node = node.shadowRoot; } ");
            js.push_str("if (!node) return null; ");
        }
    }

    js.push_str(&format!("var el = node.querySelector('{}'); ", final_selector));
    js.push_str("if (!el) return null; ");

    match action {
        "type" => {
            js.push_str(&format!("el.value = '{}'; el.dispatchEvent(new Event('input', {{bubbles: true}})); ", value.replace('\'', "\\'")));
        }
        "click" => {
            js.push_str("el.click(); ");
        }
        "hover" => {
            js.push_str("if (el.dispatchEvent) { el.dispatchEvent(new MouseEvent('mouseover', {bubbles: true, cancelable: true, view: window})); } ");
        }
        "read" => {
            js.push_str("return el.textContent || el.innerText || ''; ");
        }
        "read_html" => {
            js.push_str("return el.innerHTML || ''; ");
        }
        "select" => {
            js.push_str(&format!("var opts = el.querySelectorAll('option'); for (var i=0;i<opts.length;i++) {{ var opt = opts[i]; if (opt.textContent === '{}' || opt.value === '{}') {{ opt.selected = true; }} }} el.dispatchEvent(new Event('change', {{bubbles: true}})); ", value.replace('\'', "\\'"), value.replace('\'', "\\'")));
        }
        "check" => {
            js.push_str("if (el.type === 'checkbox' || el.type === 'radio') { el.checked = true; el.dispatchEvent(new Event('change', {bubbles: true})); } ");
        }
        "uncheck" => {
            js.push_str("if (el.type === 'checkbox' || el.type === 'radio') { el.checked = false; el.dispatchEvent(new Event('change', {bubbles: true})); } ");
        }
        "dblclick" => {
            js.push_str("var evt = new MouseEvent('dblclick', {bubbles: true, cancelable: true, view: window}); el.dispatchEvent(evt); ");
        }
        "right_click" => {
            js.push_str("var evt = new MouseEvent('contextmenu', {bubbles: true, cancelable: true, view: window}); el.dispatchEvent(evt); ");
        }
        "clear" => {
            js.push_str("el.value = ''; el.dispatchEvent(new Event('input', {bubbles: true}})); ");
        }
        "scroll" => {
            js.push_str("el.scrollIntoView({behavior: 'smooth', block: 'center'}); ");
        }
        "attribute" => {
            js.push_str(&format!("return el.getAttribute('{}') || ''; ", value));
        }
        "value" => {
            js.push_str("return el.value || ''; ");
        }
        _ => {}
    }

    js.push_str("})()");
    js
}

async fn do_click(
    page: &Arc<ferridriver::Page>,
    target: &Option<String>,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!("[EXEC] click: selector={}, shadow_root={:?}, iframe={:?}", selector_str, shadow_root, iframe);
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            page.click(&selector_str, None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to click: {}", e)))?;
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "click", "");
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to click via shadow DOM: {}", e)))?;
        }
    }
    Ok(())
}

async fn do_hover(
    page: &Arc<ferridriver::Page>,
    target: &Option<String>,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!("[EXEC] hover: selector={}, shadow_root={:?}, iframe={:?}", selector_str, shadow_root, iframe);
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            page.hover(&selector_str, None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to hover: {}", e)))?;
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "hover", "");
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to hover via shadow DOM: {}", e)))?;
        }
    }
    Ok(())
}

async fn do_wait(
    page: &Arc<ferridriver::Page>,
    context: &mut Context,
    url: &Option<String>,
    target: &Option<String>,
    duration: &Option<u64>,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    if let Some(ms) = duration {
        if verbose {
            eprintln!("[EXEC] wait: duration={}ms", ms);
        }
        tokio::time::sleep(Duration::from_millis(*ms)).await;
    } else {
        ensure_page_initialized(page).await?;
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);
        if let Some(url) = url {
            let resolved_url = context.resolve_with_url(url, Some(&page.url()));
            if verbose {
                eprintln!("[EXEC] wait: url={}, timeout={:?}", resolved_url, timeout);
            }
            wait_for_url(page, &resolved_url, timeout).await?;
        } else {
            let sel = selector::parse_selector(target.clone())?;
            if let Some(selector) = &sel {
                let selector_str = selector_to_string(selector);
                if verbose {
                    eprintln!("[EXEC] wait: selector={}, timeout={:?}, shadow_root={:?}, iframe={:?}", selector_str, timeout, shadow_root, iframe);
                }

                let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

                if !has_shadow_or_iframe {
                    wait_for_selector(page, &selector_str, timeout).await?;
                } else {
                    let final_selector = selector_str.trim_start_matches("xpath=");
                    let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "read", "");
                    page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                        .await
                        .map_err(|e| Error::ActionFailed(format!("Failed to wait via shadow DOM: {}", e)))?;
                }
            }
        }
    }
    Ok(())
}

async fn do_read(
    page: &Arc<ferridriver::Page>,
    context: &mut Context,
    target: &Option<String>,
    to: &Option<String>,
    html: bool,
    attribute: Option<&str>,
    read_value: bool,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    ensure_page_initialized(page).await?;
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!(
                "[EXEC] read: selector={}, to={:?}, html={}, attribute={:?}, value={}, shadow_root={:?}, iframe={:?}",
                selector_str, to, html, attribute, read_value, shadow_root, iframe
            );
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        let content = if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            let locator = page.locator(&selector_str, None);
            if let Some(attr) = attribute {
                locator.get_attribute(attr)
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to read attribute '{}': {}", attr, e)))?
                    .unwrap_or_default()
            } else if read_value {
                locator.input_value()
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to read input value: {}", e)))?
            } else if html {
                locator.inner_html()
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to read html: {}", e)))?
            } else {
                locator.inner_text()
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to read text: {}", e)))?
            }
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let action = if attribute.is_some() {
                "attribute"
            } else if read_value {
                "value"
            } else if html {
                "read_html"
            } else {
                "read"
            };
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, action, attribute.unwrap_or(""));
            let result = page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to read via shadow DOM: {}", e)))?;
            let content = match result {
                ferridriver::protocol::SerializedValue::Str(s) => s,
                ferridriver::protocol::SerializedValue::Number(n) => n.to_string(),
                ferridriver::protocol::SerializedValue::Bool(b) => b.to_string(),
                ferridriver::protocol::SerializedValue::Special(ferridriver::protocol::SpecialValue::Null) => String::new(),
                _ => String::new(),
            };
            content
        };

        if let Some(var_name) = to {
            context.set(var_name, &content);
        } else {
            println!("{}", content);
        }
    }
    Ok(())
}

async fn do_js(
    page: &Arc<ferridriver::Page>,
    context: &mut Context,
    code: &str,
    to: &Option<String>,
    verbose: bool,
) -> Result<(), Error> {
    ensure_page_initialized(page).await?;
    let result = page
        .evaluate(
            code,
            ferridriver::protocol::SerializedArgument::default(),
            None,
        )
        .await
        .map_err(|e| Error::ActionFailed(format!("Failed to evaluate js: {}", e)))?;
    let result_str = match result {
        ferridriver::protocol::SerializedValue::Str(s) => s,
        ferridriver::protocol::SerializedValue::Number(n) => n.to_string(),
        ferridriver::protocol::SerializedValue::Bool(b) => b.to_string(),
        ferridriver::protocol::SerializedValue::Special(s) => format!("{:?}", s),
        _ => format!("{:?}", result),
    };
    if verbose {
        eprintln!("[EXEC] js: to={:?}, result={}", to, result_str);
    }
    if let Some(var_name) = to {
        context.set(var_name, &result_str);
    } else {
        println!("{}", result_str);
    }
    Ok(())
}

async fn do_select(
    page: &Arc<ferridriver::Page>,
    context: &mut Context,
    value: &str,
    target: &Option<String>,
    values: &[String],
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let resolved_value = context.resolve_value_with_url(value, Some(&page.url()))?;
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!(
                "[EXEC] select: value={}, selector={}, shadow_root={:?}, iframe={:?}",
                resolved_value, selector_str, shadow_root, iframe
            );
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            let option_values: Vec<SelectOptionValue> = if values.is_empty() {
                vec![SelectOptionValue::by_value(resolved_value.clone())]
            } else {
                values.iter().map(|v| {
                    let resolved = context.resolve(v);
                    SelectOptionValue::by_value(resolved.clone())
                }).collect()
            };
            page.select_option(&selector_str, option_values, Some(SelectOptionOptions::default()))
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to select option: {}", e)))?;
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let values_json = if values.is_empty() {
                resolved_value.clone()
            } else {
                values.join(",")
            };
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "select", &values_json);
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to select via shadow DOM: {}", e)))?;
        }
    }
    Ok(())
}

async fn do_check(
    page: &Arc<ferridriver::Page>,
    target: &Option<String>,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    check: bool,
    verbose: bool,
) -> Result<(), Error> {
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!(
                "[EXEC] {}: selector={}, shadow_root={:?}, iframe={:?}",
                if check { "check" } else { "uncheck" },
                selector_str, shadow_root, iframe
            );
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            if check {
                page.check(&selector_str, None)
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to check: {}", e)))?
            } else {
                page.uncheck(&selector_str, None)
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to uncheck: {}", e)))?
            }
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let action = if check { "check" } else { "uncheck" };
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, action, "");
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to {} via shadow DOM: {}", action, e)))?;
        }
    }
    Ok(())
}

async fn do_dblclick(
    page: &Arc<ferridriver::Page>,
    target: &Option<String>,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!(
                "[EXEC] dblclick: selector={}, shadow_root={:?}, iframe={:?}",
                selector_str, shadow_root, iframe
            );
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            page.dblclick(&selector_str, None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to dblclick: {}", e)))?
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "dblclick", "");
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to dblclick via shadow DOM: {}", e)))?;
        }
    }
    Ok(())
}

async fn do_right_click(
    page: &Arc<ferridriver::Page>,
    target: &Option<String>,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!(
                "[EXEC] right_click: selector={}, shadow_root={:?}, iframe={:?}",
                selector_str, shadow_root, iframe
            );
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            page.locator(&selector_str, None)
                .right_click()
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to right_click: {}", e)))?
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "right_click", "");
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to right_click via shadow DOM: {}", e)))?;
        }
    }
    Ok(())
}

async fn do_clear(
    page: &Arc<ferridriver::Page>,
    target: &Option<String>,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!(
                "[EXEC] clear: selector={}, shadow_root={:?}, iframe={:?}",
                selector_str, shadow_root, iframe
            );
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            page.locator(&selector_str, None)
                .clear()
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to clear: {}", e)))?
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "clear", "");
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to clear via shadow DOM: {}", e)))?;
        }
    }
    Ok(())
}

async fn do_upload(
    page: &Arc<ferridriver::Page>,
    files: &[String],
    target: &Option<String>,
    _action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let sel = selector::parse_selector(target.clone())?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        if verbose {
            eprintln!(
                "[EXEC] upload: files={:?}, selector={}, shadow_root={:?}, iframe={:?}",
                files, selector_str, shadow_root, iframe
            );
        }
        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();
        if has_shadow_or_iframe {
            return Err(Error::ActionFailed("upload action does not support shadow_root or iframe".to_string()));
        }

        let paths: Vec<std::path::PathBuf> = files.iter()
            .map(|f| std::path::PathBuf::from(f))
            .collect();
        page.set_input_files(&selector_str, InputFiles::Paths(paths), Some(SetInputFilesOptions::default()))
            .await
            .map_err(|e| Error::ActionFailed(format!("Failed to upload files: {}", e)))?
    }
    Ok(())
}

async fn do_drag(
    page: &Arc<ferridriver::Page>,
    context: &mut Context,
    source: &str,
    to: &str,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let resolved_source = context.resolve_value_with_url(source, Some(&page.url()))?;
    let resolved_to = context.resolve_value_with_url(to, Some(&page.url()))?;
    let sel_src = selector::parse_selector(Some(resolved_source.clone()))?;
    let sel_dst = selector::parse_selector(Some(resolved_to.clone()))?;
    if let (Some(src_selector), Some(dst_selector)) = (&sel_src, &sel_dst) {
        ensure_page_initialized(page).await?;
        let src_str = selector_to_string(src_selector);
        let dst_str = selector_to_string(dst_selector);
        if verbose {
            eprintln!(
                "[EXEC] drag: source={}, to={}, shadow_root={:?}, iframe={:?}",
                src_str, dst_str, shadow_root, iframe
            );
        }
        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();
        if has_shadow_or_iframe {
            return Err(Error::ActionFailed("drag action does not support shadow_root or iframe".to_string()));
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);
        wait_for_element(page, &src_str, timeout).await?;
        wait_for_element(page, &dst_str, timeout).await?;
        page.drag_and_drop(&src_str, &dst_str, None)
            .await
            .map_err(|e| Error::ActionFailed(format!("Failed to drag and drop: {}", e)))?
    }
    Ok(())
}

async fn do_scroll(
    page: &Arc<ferridriver::Page>,
    context: &mut Context,
    target: &str,
    x: Option<i64>,
    y: Option<i64>,
    action_timeout: Option<u64>,
    shadow_root: &Option<SelectorPath>,
    iframe: &Option<SelectorPath>,
    verbose: bool,
) -> Result<(), Error> {
    let resolved_target = context.resolve_value_with_url(target, Some(&page.url()))?;
    let sel = selector::parse_selector(Some(resolved_target.clone()))?;
    if let Some(selector) = &sel {
        ensure_page_initialized(page).await?;
        let selector_str = selector_to_string(selector);
        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        if !has_shadow_or_iframe {
            let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);
            wait_for_element(page, &selector_str, timeout).await?;
            page.locator(&selector_str, None)
                .scroll_into_view_if_needed()
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to scroll element into view: {}", e)))?;

            if let (Some(dx), Some(dy)) = (x, y) {
                page.mouse().wheel(dx as f64, dy as f64)
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to wheel scroll: {}", e)))?;
            }
        } else {
            if x.is_some() || y.is_some() {
                return Err(Error::ActionFailed("scroll with x/y does not support shadow_root or iframe".to_string()));
            }
            let final_selector = selector_str.trim_start_matches("xpath=");
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, "scroll", "");
            page.evaluate(&js_expr, ferridriver::protocol::SerializedArgument::default(), None)
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to scroll via shadow DOM: {}", e)))?;
        }

        if verbose {
            eprintln!(
                "[EXEC] scroll: target={}, x={:?}, y={:?}, shadow_root={:?}, iframe={:?}",
                selector_str, x, y, shadow_root, iframe
            );
        }
    }
    Ok(())
}

async fn wait_for_url(
    page: &Arc<ferridriver::Page>,
    url: &str,
    timeout: Duration,
) -> Result<(), Error> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let current_url = page.url();
        if current_url.contains(url) || url.contains(&current_url) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(Error::Timeout(format!("URL: {}", url)))
}

async fn wait_for_selector(
    page: &Arc<ferridriver::Page>,
    selector: &str,
    timeout: Duration,
) -> Result<(), Error> {
    let timeout_ms = timeout.as_millis() as u64;
    page.wait_for_selector(
        selector,
        ferridriver::options::WaitOptions {
            state: Some("attached".to_string()),
            timeout: Some(timeout_ms),
        },
    )
    .await
    .map_err(|e| Error::Timeout(format!("Selector: {} - {}", selector, e)))?;
    Ok(())
}

async fn wait_for_element(
    page: &Arc<ferridriver::Page>,
    selector: &str,
    timeout: Duration,
) -> Result<(), Error> {
    wait_for_selector(page, selector, timeout).await
}

async fn check_condition(
    page: &Arc<ferridriver::Page>,
    selector: Option<&Selector>,
    _verbose: bool,
) -> Result<bool, Error> {
    ensure_page_initialized(page).await?;
    match selector {
        Some(Selector::XPath(x)) => {
            let selector_str = format!("xpath={}", x);
            let count = page
                .locator(&selector_str, None)
                .count()
                .await
                .map_err(|e| {
                    if e.to_string().contains("main_frame") || e.to_string().contains("navigation")
                    {
                        Error::ActionFailed(
                            "Page not initialized. Use 'goto' first to navigate to a page."
                                .to_string(),
                        )
                    } else {
                        Error::ActionFailed(format!("Failed to count xpath: {}", e))
                    }
                })?;
            Ok(count > 0)
        }
        Some(Selector::Css(c)) => {
            let count = page.locator(c, None).count().await.map_err(|e| {
                if e.to_string().contains("main_frame") || e.to_string().contains("navigation") {
                    Error::ActionFailed(
                        "Page not initialized. Use 'goto' first to navigate to a page.".to_string(),
                    )
                } else {
                    Error::ActionFailed(format!("Failed to count css: {}", e))
                }
            })?;
            Ok(count > 0)
        }
        None => Ok(false),
    }
}

async fn evaluate_condition(
    page: &Arc<ferridriver::Page>,
    condition: &Condition,
    context: &Context,
    verbose: bool,
) -> Result<bool, Error> {
    match condition {
        Condition::XPath(x) => {
            let sel = selector::parse_selector(Some(x.clone()))?;
            if verbose {
                eprintln!("[EXEC] if: xpath={}", x);
            }
            check_condition(page, sel.as_ref(), verbose).await
        }
        Condition::Css(c) => {
            let sel = selector::parse_selector(Some(c.clone()))?;
            if verbose {
                eprintln!("[EXEC] if: css={}", c);
            }
            check_condition(page, sel.as_ref(), verbose).await
        }
        Condition::Url(u) => {
            ensure_page_initialized(page).await?;
            let current_url_str = parse_builtin_variables(page, "$url").await?;
            let matches = current_url_str.contains(u) || u.contains(&current_url_str);
            if verbose {
                eprintln!(
                    "[EXEC] if: url={}, current_url={}, matches={}",
                    u, current_url_str, matches
                );
            }
            Ok(matches)
        }
        Condition::Defined(var) => {
            let is_defined = context.is_defined(var);
            if verbose {
                eprintln!("[EXEC] if: defined={}, result={}", var, is_defined);
            }
            Ok(is_defined)
        }
        Condition::Not(inner) => {
            let inner_result = Box::pin(evaluate_condition(page, inner, context, verbose)).await?;
            if verbose {
                eprintln!("[EXEC] if: not={}", !inner_result);
            }
            Ok(!inner_result)
        }
        Condition::Text { target, contains } => {
            let sel = selector::parse_selector(Some(target.clone()))?;
            if let Some(sel) = &sel {
                let selector_str = selector_to_string(sel);
                wait_for_element(page, &selector_str, get_timeout()).await.ok();
                let text = page.locator(&selector_str, None).inner_text().await.unwrap_or_default();
                let matches = text.contains(contains);
                if verbose {
                    eprintln!("[EXEC] if: text target={}, contains={}, text={}, matches={}", target, contains, text, matches);
                }
                Ok(matches)
            } else {
                Ok(false)
            }
        }
        Condition::Visible(sel_opt) => {
            if let Some(sel_str) = sel_opt {
                let parsed = selector::parse_selector(Some(sel_str.clone())).ok().flatten();
                if let Some(sel) = parsed.as_ref() {
                    let selector_str = selector_to_string(sel);
                    let visible = page.locator(&selector_str, None).is_visible().await.unwrap_or(false);
                    if verbose {
                        eprintln!("[EXEC] if: visible={}, selector={}", visible, selector_str);
                    }
                    Ok(visible)
                } else {
                    Ok(false)
                }
            } else {
                Ok(false)
            }
        }
        Condition::Checked(sel_opt) => {
            if let Some(sel_str) = sel_opt {
                let parsed = selector::parse_selector(Some(sel_str.clone())).ok().flatten();
                if let Some(sel) = parsed.as_ref() {
                    let selector_str = selector_to_string(sel);
                    let checked = page.locator(&selector_str, None).is_checked().await.unwrap_or(false);
                    if verbose {
                        eprintln!("[EXEC] if: checked={}, selector={}", checked, selector_str);
                    }
                    Ok(checked)
                } else {
                    Ok(false)
                }
            } else {
                Ok(false)
            }
        }
        Condition::Enabled(sel_opt) => {
            if let Some(sel_str) = sel_opt {
                let parsed = selector::parse_selector(Some(sel_str.clone())).ok().flatten();
                if let Some(sel) = parsed.as_ref() {
                    let selector_str = selector_to_string(sel);
                    let enabled = page.locator(&selector_str, None).is_enabled().await.unwrap_or(false);
                    if verbose {
                        eprintln!("[EXEC] if: enabled={}, selector={}", enabled, selector_str);
                    }
                    Ok(enabled)
                } else {
                    Ok(false)
                }
            } else {
                Ok(false)
            }
        }
        Condition::Equals { var, value } => {
            let resolved = context.resolve(var);
            let matches = resolved == *value;
            if verbose {
                eprintln!("[EXEC] if: equals var={}, value={}, resolved={}, matches={}", var, value, resolved, matches);
            }
            Ok(matches)
        }
        Condition::And(conditions) => {
            for cond in conditions {
                let result = Box::pin(evaluate_condition(page, cond, context, verbose)).await?;
                if !result {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Condition::Or(conditions) => {
            for cond in conditions {
                let result = Box::pin(evaluate_condition(page, cond, context, verbose)).await?;
                if result {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn resolve_yaml_value(value: &serde_yaml::Value, context: &Context, current_url: Option<&str>) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::String(s) => {
            let resolved = context.resolve_with_url(s, current_url);
            if resolved != *s {
                serde_yaml::Value::String(resolved)
            } else {
                value.clone()
            }
        }
        serde_yaml::Value::Mapping(map) => {
            let mut new_map = serde_yaml::Mapping::new();
            for (k, v) in map {
                let resolved_v = resolve_yaml_value(v, context, current_url);
                new_map.insert(k.clone(), resolved_v);
            }
            serde_yaml::Value::Mapping(new_map)
        }
        serde_yaml::Value::Sequence(seq) => {
            serde_yaml::Value::Sequence(
                seq.iter().map(|v| resolve_yaml_value(v, context, current_url)).collect()
            )
        }
        _ => value.clone(),
    }
}

fn ask_user(prompt: &str) -> Result<String, Error> {
    print!("{}: ", prompt);
    std::io::Write::flush(&mut std::io::stdout())
        .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| Error::Io(e))?;
    Ok(input.trim().to_string())
}

async fn do_request(
    context: &mut Context,
    url: &str,
    method: &str,
    headers: &std::collections::HashMap<String, String>,
    body: &Option<String>,
    to: &Option<String>,
    timeout: Option<u64>,
    verbose: bool,
) -> Result<(), Error> {
    let resolved_url = context.resolve(url);
    if verbose {
        eprintln!("[EXEC] request: url={}, method={}", resolved_url, method);
    }

    let client = Client::builder()
        .timeout(timeout.map(Duration::from_millis).unwrap_or(Duration::from_secs(30)))
        .build()
        .map_err(|e| Error::ActionFailed(format!("Failed to create HTTP client: {}", e)))?;

    let mut req_builder = match method.to_uppercase().as_str() {
        "GET" => client.get(&resolved_url),
        "POST" => client.post(&resolved_url),
        "PUT" => client.put(&resolved_url),
        "DELETE" => client.delete(&resolved_url),
        "PATCH" => client.patch(&resolved_url),
        "HEAD" => client.head(&resolved_url),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, &resolved_url),
        _ => return Err(Error::ActionFailed(format!("Unsupported HTTP method: {}", method))),
    };

    for (k, v) in headers {
        let resolved_val = context.resolve(v);
        req_builder = req_builder.header(k, resolved_val);
    }

    if let Some(body) = body {
        let resolved_body = context.resolve(body);
        req_builder = req_builder.body(resolved_body);
    }

    let response = req_builder.send().await
        .map_err(|e| Error::ActionFailed(format!("Request failed: {}", e)))?;

    let status = response.status();
    let body = response.text().await
        .map_err(|e| Error::ActionFailed(format!("Failed to read response body: {}", e)))?;

    if verbose {
        eprintln!("[EXEC] request: status={}, body_len={}", status, body.len());
    }

    let yaml_value: serde_yaml::Value = match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(json_val) => {
            let json_str = serde_json::to_string(&json_val).unwrap_or_default();
            serde_yaml::from_str(&json_str).unwrap_or_else(|_| serde_yaml::Value::String(body))
        }
        Err(_) => serde_yaml::Value::String(body),
    };

    if let Some(var_name) = to {
        context.set_yaml(var_name, yaml_value);
    } else {
        if let Some(yaml_str) = yaml_value.as_str() {
            println!("{}", yaml_str);
        } else {
            println!("{}", serde_yaml::to_string(&yaml_value).unwrap_or_default());
        }
    }

    Ok(())
}

async fn run_server(
    base_path: String,
    port: u16,
    ws_url: Option<String>,
    routes: std::collections::HashMap<String, crate::flow::RouteConfig>,
    flows: crate::flow::FlowFile,
    verbose: bool,
) -> Result<(), Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let addr = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| Error::ActionFailed(format!("Failed to bind to {}: {}", addr, e)))?;

    if verbose {
        eprintln!("[SERVER] listening on {}", addr);
        eprintln!("[SERVER] routes: {:?}", routes);
    }

    loop {
        match listener.accept().await {
            Ok((mut stream, _)) => {
                let base_path = base_path.clone();
                let routes = routes.clone();
                let flows = flows.clone();
                let ws_url = ws_url.clone();
                let verbose = verbose;

                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    let mut request_line = String::new();
                    let mut headers = Vec::new();
                    let mut content_length = 0;
                    let mut header_end_offset = 0;
                    let mut bytes_read_total = 0;

                    loop {
                        match stream.read(&mut buf[bytes_read_total..]).await {
                            Ok(0) => break,
                            Ok(n) => {
                                bytes_read_total += n;
                                let text = String::from_utf8_lossy(&buf[..bytes_read_total]);
                                if let Some(end_pos) = text.find("\r\n\r\n") {
                                    header_end_offset = end_pos + 4;
                                    let header_text = &text[..end_pos];
                                    for line in header_text.lines() {
                                        if request_line.is_empty() {
                                            request_line = line.to_string();
                                        } else {
                                            headers.push(line.to_string());
                                            if line.to_lowercase().starts_with("content-length:") {
                                                if let Ok(len) = line.split(':').nth(1).unwrap_or("0").trim().parse::<usize>() {
                                                    content_length = len;
                                                }
                                            }
                                        }
                                    }
                                    break;
                                }
                            }
                            Err(e) => {
                                eprintln!("[SERVER] read error: {}", e);
                                return;
                            }
                        }
                    }

                    let parts: Vec<&str> = request_line.split_whitespace().collect();
                    if parts.len() < 2 {
                        return;
                    }

                    let method = parts[0].to_string();
                    let path = parts[1].to_string();

                    let (path_without_query, query_string) = if let Some(q_pos) = path.find('?') {
                        (path[..q_pos].to_string(), Some(path[q_pos+1..].to_string()))
                    } else {
                        (path.clone(), None)
                    };

                    let route_path = if path_without_query.starts_with(&base_path) {
                        path_without_query[base_path.len()..].to_string()
                    } else {
                        path_without_query
                    };

                    let mut body = String::new();
                    if content_length > 0 && header_end_offset > 0 {
                        let body_start = header_end_offset;
                        let body_end = (header_end_offset + content_length).min(bytes_read_total);
                        if body_start < bytes_read_total {
                            body = String::from_utf8_lossy(&buf[body_start..body_end]).to_string();
                        }
                    }

                    let route_config = routes.get(&route_path);
                    let flow_name: Option<String> = match route_config {
                        Some(cfg) => {
                            match method.as_str() {
                                "GET" => cfg.get.clone().or(cfg.default.clone()),
                                "POST" => cfg.post.clone().or(cfg.default.clone()),
                                _ => cfg.default.clone(),
                            }
                        }
                        None => None,
                    };

                    let response = if let Some(flow_name) = flow_name {
                        let mut ctx = crate::flow::Context::new();

                        let mut request_map = serde_yaml::Mapping::new();
                        request_map.insert(serde_yaml::Value::String("method".to_string()), serde_yaml::Value::String(method.clone()));
                        request_map.insert(serde_yaml::Value::String("path".to_string()), serde_yaml::Value::String(route_path.clone()));
                        request_map.insert(serde_yaml::Value::String("base_path".to_string()), serde_yaml::Value::String(base_path.clone()));

                        let mut headers_map = serde_yaml::Mapping::new();
                        for header in &headers {
                            if let Some((key, value)) = header.split_once(':') {
                                headers_map.insert(
                                    serde_yaml::Value::String(key.trim().to_string()),
                                    serde_yaml::Value::String(value.trim().to_string()),
                                );
                            }
                        }
                        request_map.insert(serde_yaml::Value::String("headers".to_string()), serde_yaml::Value::Mapping(headers_map));

                        if let Some(qs) = &query_string {
                            let mut query_map = serde_yaml::Mapping::new();
                            for pair in qs.split('&') {
                                let parts: Vec<&str> = pair.splitn(2, '=').collect();
                                if parts.len() == 2 {
                                    query_map.insert(
                                        serde_yaml::Value::String(parts[0].to_string()),
                                        serde_yaml::Value::String(parts[1].to_string()),
                                    );
                                }
                            }
                            request_map.insert(serde_yaml::Value::String("query_params".to_string()), serde_yaml::Value::Mapping(query_map));
                        }

                        if !body.is_empty() {
                            let looks_like_json = body.trim().starts_with('{') || body.trim().starts_with('[');
                            if looks_like_json {
                                if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&body) {
                                    if let Ok(yaml_value) = serde_yaml::to_value(json_value) {
                                        request_map.insert(serde_yaml::Value::String("body".to_string()), yaml_value);
                                    } else {
                                        request_map.insert(serde_yaml::Value::String("body".to_string()), serde_yaml::Value::String(body.clone()));
                                    }
                                } else {
                                    let mut body_map = serde_yaml::Mapping::new();
                                    for pair in body.split('&') {
                                        let parts: Vec<&str> = pair.splitn(2, '=').collect();
                                        if parts.len() == 2 {
                                            body_map.insert(
                                                serde_yaml::Value::String(parts[0].to_string()),
                                                serde_yaml::Value::String(parts[1].to_string()),
                                            );
                                        }
                                    }
                                    if body_map.is_empty() {
                                        request_map.insert(serde_yaml::Value::String("body".to_string()), serde_yaml::Value::String(body.clone()));
                                    } else {
                                        request_map.insert(serde_yaml::Value::String("body".to_string()), serde_yaml::Value::Mapping(body_map));
                                    }
                                }
                            } else {
                                let mut body_map = serde_yaml::Mapping::new();
                                for pair in body.split('&') {
                                    let parts: Vec<&str> = pair.splitn(2, '=').collect();
                                    if parts.len() == 2 {
                                        body_map.insert(
                                            serde_yaml::Value::String(parts[0].to_string()),
                                            serde_yaml::Value::String(parts[1].to_string()),
                                        );
                                    }
                                }
                                if body_map.is_empty() {
                                    request_map.insert(serde_yaml::Value::String("body".to_string()), serde_yaml::Value::String(body.clone()));
                                } else {
                                    request_map.insert(serde_yaml::Value::String("body".to_string()), serde_yaml::Value::Mapping(body_map));
                                }
                            }
                        }

                        ctx.set_yaml("request", serde_yaml::Value::Mapping(request_map));

                        let ws_url_for_actions = ws_url.clone().unwrap_or_else(|| "http://127.0.0.1:9222".to_string());
                        let result = execute_server_flow(&flow_name, &mut ctx, verbose, &flows, &ws_url_for_actions).await;

                        match result {
                            Ok(_) => {
                                let (status, response_body, response_headers) = build_response_from_context(&ctx);
                                let headers_str = if response_headers.is_empty() {
                                    "Content-Type: application/json\r\n".to_string()
                                } else {
                                    response_headers
                                };
                                format!("HTTP/1.1 {} OK\r\nContent-Type: application/json\r\n{}\r\n{}",
                                    status, headers_str, response_body)
                            }
                            Err(e) => {
                                let error_body = serde_json::json!({"error": e.to_string()}).to_string();
                                format!("HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                    error_body.len(), error_body)
                            }
                        }
                    } else {
                        let error_body = serde_json::json!({"error": "not found"}).to_string();
                        format!("HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            error_body.len(), error_body)
                    };

                    if let Err(e) = stream.write_all(response.as_bytes()).await {
                        eprintln!("[SERVER] write error: {}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("[SERVER] accept error: {}", e);
            }
        }
    }
}

async fn execute_server_flow(
    flow_name: &str,
    context: &mut crate::flow::Context,
    verbose: bool,
    flows: &crate::flow::FlowFile,
    ws_url: &str,
) -> Result<(), Error> {
    let sub_actions = flows
        .get(flow_name)
        .ok_or_else(|| Error::ActionFailed(format!("Flow '{}' not found", flow_name)))?;

    run_server_actions(sub_actions.clone(), context, verbose, flows, ws_url).await?;
    Ok(())
}

fn build_response_from_context(context: &crate::flow::Context) -> (u16, String, String) {
    let mut status = 200;
    let mut body = "{}".to_string();
    let mut headers = String::new();

    if let Some(response_val) = context.get("response") {
        if let Some(status_val) = response_val.get("status") {
            if let Some(n) = status_val.as_i64() {
                status = n as u16;
            }
        }
        if let Some(body_val) = response_val.get("body") {
            body = serde_json::to_string(body_val).unwrap_or_else(|_| "{}".to_string());
        }
        if let Some(headers_val) = response_val.get("headers") {
            if let serde_yaml::Value::Mapping(map) = headers_val {
                let mut header_parts = Vec::new();
                for (k, v) in map {
                    if let (serde_yaml::Value::String(key), serde_yaml::Value::String(value)) = (k, v) {
                        header_parts.push(format!("{}: {}\r\n", key, value));
                    }
                }
                headers = header_parts.join("");
            }
        }
    }

    (status, body, headers)
}

async fn run_server_actions(
    actions: Vec<crate::flow::Action>,
    context: &mut crate::flow::Context,
    verbose: bool,
    flows: &crate::flow::FlowFile,
    ws_url: &str,
) -> Result<String, Error> {
    let mut response_body = String::new();

    for action in actions {
        match action {
            crate::flow::Action::Log { text } => {
                let resolved = context.resolve(&text);
                if verbose {
                    eprintln!("[SERVER] log: {}", resolved);
                }
                println!("{}", resolved);
            }
            crate::flow::Action::Return { values } => {
                if values.len() == 1 {
                    if let Some((_, value)) = values.iter().next() {
                        let resolved = context.resolve(&serde_yaml::to_string(&value).unwrap_or_default());
                        response_body = resolved;
                    }
                } else {
                    let mut map = serde_json::Map::new();
                    for (key, value) in values {
                        let resolved = context.resolve(&serde_yaml::to_string(&value).unwrap_or_default());
                        if let Ok(v) = serde_json::from_str(&resolved) {
                            map.insert(key.clone(), v);
                        } else {
                            map.insert(key.clone(), serde_json::Value::String(resolved));
                        }
                    }
                    response_body = serde_json::to_string(&map).unwrap_or_default();
                }
            }
            crate::flow::Action::Define { var, value, overwrite } => {
                if overwrite.clone() || !context.is_defined(&var) {
                    let resolved_value = resolve_yaml_value(&value, context, None);
                    if verbose {
                        eprintln!("[SERVER] define: {}={:?}", var, resolved_value);
                    }
                    context.set_yaml(var, resolved_value);
                }
            }
            crate::flow::Action::If { condition, then, else_ } => {
                let condition_met = evaluate_condition_for_server(&condition, context).await;
                if verbose {
                    eprintln!("[SERVER] if: condition={}", condition_met);
                }
                let branch = if condition_met { then } else { else_ };
                let result = Box::pin(run_server_actions(branch, context, verbose, flows, ws_url)).await?;
                if !result.is_empty() && response_body.is_empty() {
                    response_body = result;
                }
            }
            crate::flow::Action::Fail { message } => {
                return Err(Error::ActionFailed(message.clone()));
            }
            crate::flow::Action::Call { flow, params } => {
                if verbose {
                    eprintln!("[SERVER] call: flow={}, params={:?}", flow, params);
                }
                let sub_actions = flows
                    .get(&flow)
                    .ok_or_else(|| Error::ActionFailed(format!("Flow '{}' not found", flow)))?;
                let mut sub_context = context.clone();
                for (key, value) in params {
                    let value_str = serde_yaml::to_string(&value).unwrap_or_default();
                    sub_context.set(key, value_str.trim());
                }
                let result = Box::pin(run_server_actions(sub_actions.clone(), &mut sub_context, verbose, flows, ws_url)).await?;
                if !result.is_empty() && response_body.is_empty() {
                    response_body = result;
                }
            }
            _ => {
                if verbose {
                    eprintln!("[SERVER] executing browser action: {:?}", action);
                }
                let ws_url = ws_url.to_string();
                let action = action.clone();
                let flows = flows.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                        let browser_actions = vec![action];
                        let mut ctx = crate::flow::Context::new();
                        let result = run_actions(&ws_url, &browser_actions, &mut ctx, verbose, &flows).await;
                        tx.send(result).ok();
                    });
                });
                if let Err(e) = rx.recv() {
                    if verbose {
                        eprintln!("[SERVER] browser action error: {}", e);
                    }
                }
            }
        }
    }

    if response_body.is_empty() {
        response_body = "{}".to_string();
    }

    Ok(response_body)
}

async fn evaluate_condition_for_server(
    condition: &crate::flow::Condition,
    context: &crate::flow::Context,
) -> bool {
    match condition {
        crate::flow::Condition::Equals { var, value } => {
            let resolved = context.resolve(&format!("${}", var));
            resolved == *value
        }
        crate::flow::Condition::Defined(var) => {
            context.is_defined(var)
        }
        _ => false,
    }
}
