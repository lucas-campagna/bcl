use crate::cdp::client::CdpClient;
use crate::error::Error;
use base64::Engine;
use regex::Regex;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum RouteHandler {
    Continue,
    Fulfill {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    Fail {
        reason: String,
    },
}

#[derive(Debug, Clone)]
struct Route {
    pattern: String,
    handler: RouteHandler,
}

#[derive(Default)]
pub struct RouteRegistry {
    routes: tokio::sync::Mutex<Vec<Route>>,
    enabled: AtomicBool,
}

impl RouteRegistry {
    pub async fn add(&self, pattern: String, handler: RouteHandler) -> Result<bool, Error> {
        let mut routes = self.routes.lock().await;
        routes.push(Route { pattern, handler });
        Ok(routes.len() == 1)
    }

    pub async fn remove(&self, pattern: &str) -> bool {
        let mut routes = self.routes.lock().await;
        let before = routes.len();
        routes.retain(|route| route.pattern != pattern);
        routes.len() != before
    }

    pub async fn is_empty(&self) -> bool {
        self.routes.lock().await.is_empty()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }

    pub fn disable(&self) -> bool {
        self.enabled.swap(false, Ordering::SeqCst)
    }

    pub async fn match_handler(&self, url: &str) -> Option<RouteHandler> {
        let routes = self.routes.lock().await;
        routes
            .iter()
            .rev()
            .find(|route| url_matches(&route.pattern, url))
            .map(|route| route.handler.clone())
    }
}

fn glob_to_regex(pattern: &str) -> Result<Regex, regex::Error> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut source = String::from("^");
    let mut index = 0;

    while index < chars.len() {
        match chars[index] {
            '*' => {
                let mut count = 0;
                while index < chars.len() && chars[index] == '*' {
                    count += 1;
                    index += 1;
                }
                if count >= 2 {
                    source.push_str(".*");
                } else {
                    source.push_str("[^/]*");
                }
                continue;
            }
            '?' => {
                source.push_str("[^/]");
            }
            character => {
                source.push_str(&regex::escape(&character.to_string()));
            }
        }
        index += 1;
    }

    source.push('$');
    Regex::new(&source)
}

fn url_matches(pattern: &str, url: &str) -> bool {
    match glob_to_regex(pattern) {
        Ok(regex) => regex.is_match(url),
        Err(_) => pattern == url,
    }
}

fn normalize_fail_reason(reason: &str) -> &'static str {
    match reason.trim().to_lowercase().as_str() {
        "abort" | "aborted" => "Aborted",
        "access" | "denied" | "accessdenied" | "access_denied" => "AccessDenied",
        "closed" => "Closed",
        "connectionaborted" | "connection_aborted" => "ConnectionAborted",
        "connectionclosed" | "connection_closed" => "ConnectionClosed",
        "connectionfailed" | "connection_failed" => "ConnectionFailed",
        "connectionrefused" | "connection_refused" => "ConnectionRefused",
        "connectionreset" | "connection_reset" => "ConnectionReset",
        "internetdisconnected" | "internet_disconnected" => "InternetDisconnected",
        "networkunreachable" | "network_unreachable" => "NetworkUnreachable",
        "timeout" | "timedout" | "timed_out" => "TimedOut",
        _ => "Failed",
    }
}

pub(crate) async fn resolve_paused_request(
    client: Arc<CdpClient>,
    request_id: String,
    handler: Option<RouteHandler>,
) {
    let result = match handler.unwrap_or(RouteHandler::Continue) {
        RouteHandler::Continue => client
            .send(
                "Fetch.continueRequest",
                Some(json!({ "requestId": request_id })),
            )
            .await
            .map(|_| ()),
        RouteHandler::Fulfill {
            status,
            headers,
            body,
        } => {
            let response_headers: Vec<serde_json::Value> = headers
                .iter()
                .map(|(name, value)| json!({ "name": name, "value": value }))
                .collect();
            let encoded_body = base64::engine::general_purpose::STANDARD.encode(body.as_bytes());
            client
                .send(
                    "Fetch.fulfillRequest",
                    Some(json!({
                        "requestId": request_id,
                        "responseCode": status,
                        "responseHeaders": response_headers,
                        "body": encoded_body,
                    })),
                )
                .await
                .map(|_| ())
        }
        RouteHandler::Fail { reason } => client
            .send(
                "Fetch.failRequest",
                Some(json!({
                    "requestId": request_id,
                    "errorReason": normalize_fail_reason(&reason),
                })),
            )
            .await
            .map(|_| ()),
    };

    if result.is_err() {
        // A paused request must always be resolved or navigation can hang.
        let _ = client
            .send(
                "Fetch.continueRequest",
                Some(json!({ "requestId": request_id })),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_matches_supports_glob_patterns() {
        assert!(url_matches("https://example.com/api/**", "https://example.com/api/users/1"));
        assert!(url_matches("https://example.com/api/*", "https://example.com/api/users"));
        assert!(!url_matches("https://example.com/api/*", "https://example.com/api/users/1"));
        assert!(url_matches("**/logo.png", "https://cdn.example.com/assets/logo.png"));
        assert!(url_matches("https://example.com/a?c", "https://example.com/abc"));
        assert!(!url_matches("https://example.com/a?c", "https://example.com/ab/c"));
        assert!(url_matches("https://example.com", "https://example.com"));
    }

    #[test]
    fn fail_reasons_are_normalized_to_cdp_values() {
        assert_eq!(normalize_fail_reason("abort"), "Aborted");
        assert_eq!(normalize_fail_reason("TIMED_OUT"), "TimedOut");
        assert_eq!(normalize_fail_reason("something else"), "Failed");
    }
}
