use crate::cdp::client::CdpClient;
use crate::error::Error;
use serde_json::json;
use std::sync::Arc;

pub struct Dialog {
    client: Arc<CdpClient>,
}

impl Dialog {
    pub fn new(client: Arc<CdpClient>) -> Self {
        Dialog { client }
    }

    pub async fn accept(&self, prompt_text: Option<String>) -> Result<(), Error> {
        self.client
            .send(
                "Page.handleJavaScriptDialog",
                Some(json!({ "accept": true, "promptText": prompt_text })),
            )
            .await?;
        Ok(())
    }

    pub async fn dismiss(&self) -> Result<(), Error> {
        self.client
            .send(
                "Page.handleJavaScriptDialog",
                Some(json!({ "accept": false })),
            )
            .await?;
        Ok(())
    }
}
