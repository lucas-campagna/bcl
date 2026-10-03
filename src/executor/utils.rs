use crate::error::Error;
use std::sync::Arc;
use std::time::Duration;

pub fn get_timeout() -> Duration {
    std::env::var("TIMEOUT")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_millis(180_000))
}

pub async fn parse_builtin_variables(
    page: &Arc<ferridriver::Page>,
    input: &str,
) -> Result<String, Error> {
    let mut result = input.to_string();
    if input.contains("$url") {
        if page.url().is_empty() {
            return Ok(result);
        }
        match page
            .evaluate(
                "window.location.href",
                ferridriver::protocol::SerializedArgument::default(),
                None,
            )
            .await
        {
            Ok(current_url) => {
                if let ferridriver::protocol::SerializedValue::Str(current_url_str) = current_url {
                    result = result.replace("$url", &current_url_str);
                }
            }
            Err(_) => {
                // Page not initialized, keep $url as-is
            }
        }
    }
    Ok(result)
}

pub fn page_is_navigated(page: &Arc<ferridriver::Page>) -> bool {
    !page.url().is_empty()
}

pub async fn ensure_page_initialized(page: &Arc<ferridriver::Page>) -> Result<(), Error> {
    if page.url().is_empty() {
        page.reload(None)
            .await
            .map_err(|e| Error::ActionFailed(format!("Failed to reload page: {}", e)))?;
    }
    Ok(())
}
