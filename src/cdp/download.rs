use crate::cdp::client::CdpClient;
use crate::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Download {
    client: Arc<CdpClient>,
    guid: String,
    suggested_filename: String,
}

impl Download {
    pub fn new(client: Arc<CdpClient>, guid: String, suggested_filename: String) -> Self {
        Download {
            client,
            guid,
            suggested_filename,
        }
    }

    pub async fn save_as(&self, target: &Path) -> Result<(), Error> {
        let start = Instant::now();
        loop {
            match self.client.download_state(&self.guid).await {
                Some(state) => match state.state.as_str() {
                    "completed" => break,
                    "canceled" => {
                        return Err(Error::ActionFailed("download was canceled".to_string()))
                    }
                    _ => {}
                },
                None => {}
            }

            if start.elapsed() >= Duration::from_secs(60) {
                return Err(Error::Timeout("download".to_string()));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let dir = self.client.download_dir();
        let candidates = [
            dir.join(&self.suggested_filename),
            dir.join(&self.guid),
        ];
        let source = candidates
            .iter()
            .find(|path| path.exists())
            .ok_or_else(|| {
                Error::ActionFailed(format!(
                    "Downloaded file not found in {}",
                    dir.display()
                ))
            })?;

        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        std::fs::copy(source, target)
            .map_err(|e| Error::ActionFailed(format!("Failed to save download: {}", e)))?;
        Ok(())
    }
}
