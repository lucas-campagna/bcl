use crate::error::Error;
use crate::flow::{Action, Condition, Context, FlowFile, Selector, SelectorPath};
use crate::executor::selector;
use crate::executor::utils::{get_timeout, parse_builtin_variables, ensure_page_initialized};
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

    let page = browser
        .page()
        .await
        .map_err(|e| Error::ConnectionFailed(format!("Failed to get page: {}", e)))?;

    page.set_default_navigation_timeout(5000);

    for action in actions {
        let start = Instant::now();
        match action {
            Action::Goto { url, wait_until } => {
                let resolved_url = context.resolve_with_url(url, Some(&page.url()));
                if verbose {
                    eprintln!("[EXEC] goto: url={}", resolved_url);
                }
                let wait_until = wait_until.clone().unwrap_or_else(|| "commit".to_string());
                page.goto(
                    &resolved_url,
                    Some(ferridriver::options::GotoOptions {
                        wait_until: Some(wait_until),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|e| Error::ActionFailed(format!("Failed to goto URL: {}", e)))?;
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
                let result = do_type(&page, context, value, target, *secret, *action_timeout, shadow_root, iframe, verbose).await;
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Click { target, timeout: action_timeout, on_error, on_timeout, shadow_root, iframe, .. } => {
                let result = do_click(&page, target, *action_timeout, shadow_root, iframe, verbose).await;
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Press { key } => {
                let resolved_key = context.resolve_with_url(key, Some(&page.url()));
                if verbose {
                    eprintln!("[EXEC] press: key={}", resolved_key);
                }
                page.keyboard()
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
                let result = do_wait(&page, context, url, target, duration, *action_timeout, shadow_root, iframe, verbose).await;
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
                let result = do_hover(&page, target, *action_timeout, shadow_root, iframe, verbose).await;
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::If {
                condition,
                then,
                else_,
            } => {
                let condition_met = evaluate_condition(&page, condition, context, verbose).await?;
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
                timeout: action_timeout,
                on_error,
                on_timeout,
                shadow_root,
                iframe,
            } => {
                let result = do_read(&page, context, target, to, *html, *action_timeout, shadow_root, iframe, verbose).await;
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, on_timeout, context, verbose, flows).await?;
                }
            }
            Action::Log { text } => {
                let resolved_text = parse_builtin_variables(&page, text).await?;
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
                let result = do_js(&page, context, code, to, verbose).await;
                if let Err(e) = result {
                    handle_action_error(ws_url, e, on_error, &None, context, verbose, flows).await?;
                }
            }
            Action::Define { var, value, overwrite } => {
                if *overwrite || !context.is_defined(var) {
                    let resolved_value = resolve_yaml_value(value, context, Some(&page.url()));
                    if verbose {
                        eprintln!("[EXEC] define: {}={:?}", var, resolved_value);
                    }
                    context.set_yaml(var, resolved_value);
                } else if verbose {
                    eprintln!("[EXEC] define: {} (skipped, already defined)", var);
                }
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
                "[EXEC] read: selector={}, to={:?}, html={}, shadow_root={:?}, iframe={:?}",
                selector_str, to, html, shadow_root, iframe
            );
        }
        let timeout = action_timeout.map(Duration::from_millis).unwrap_or_else(get_timeout);

        let has_shadow_or_iframe = shadow_root.is_some() || iframe.is_some();

        let content = if !has_shadow_or_iframe {
            wait_for_element(page, &selector_str, timeout).await?;
            if html {
                page.locator(&selector_str, None)
                    .inner_html()
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to read html: {}", e)))?
            } else {
                page.locator(&selector_str, None)
                    .inner_text()
                    .await
                    .map_err(|e| Error::ActionFailed(format!("Failed to read text: {}", e)))?
            }
        } else {
            let final_selector = selector_str.trim_start_matches("xpath=");
            let action = if html { "read_html" } else { "read" };
            let js_expr = build_shadow_dom_js_for_action(shadow_root, iframe, final_selector, action, "");
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
