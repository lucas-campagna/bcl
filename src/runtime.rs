use crate::error::Error;
use crate::executor::Context;
use crate::flow::FlowFile;
use crate::flow::parser::parse_file;
use std::path::Path;

pub struct FlowRuntime {
    ws_url: String,
}

impl FlowRuntime {
    pub async fn connect(ws_url: &str) -> Result<Self, Error> {
        Ok(Self {
            ws_url: ws_url.to_string(),
        })
    }

    pub async fn execute_flow<P: AsRef<Path>>(&self, flow_path: P, _flow_name: &str) -> Result<(), Error> {
        let _flow_file = parse_file(flow_path)?;
        Err(Error::ConnectionFailed("Use run_actions directly with CDP URL".to_string()))
    }

    pub async fn execute(&self, _flow: &FlowFile, _flow_name: &str) -> Result<(), Error> {
        Err(Error::ConnectionFailed("Use run_actions directly with CDP URL".to_string()))
    }

    pub async fn execute_with_context(&self, _flow: &FlowFile, _flow_name: &str, _context: &mut Context) -> Result<(), Error> {
        Err(Error::ConnectionFailed("Use run_actions directly with CDP URL".to_string()))
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }
}

pub async fn create_runtime(ws_url: &str) -> Result<FlowRuntime, Error> {
    FlowRuntime::connect(ws_url).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::run_actions;
    use crate::flow::{Action, FlowFile};
    use std::collections::HashMap;

    #[test]
    fn test_parse_clear_flow() {
        let flow_file = parse_file("/home/lucas/projects/mybroker/assets/brokers/clear.yml").unwrap();
        assert!(!flow_file.flows.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_type_distributes_chars_to_multiple_inputs() {
        let cdp_url = "http://127.0.0.1:9222";
        let mut context = Context::new();
        context.set("CODE", "ABC");

        let html = r#"data:text/html,<html><body>
            <input type="text" id="i1" maxlength="1">
            <input type="text" id="i2" maxlength="1">
            <input type="text" id="i3" maxlength="1">
        </body></html>"#;

        let actions = vec![
            Action::Goto { url: html.to_string(), wait_until: Some("commit".to_string()) },
            Action::Type {
                value: "$CODE".to_string(),
                target: Some("//input".to_string()),
                secret: false,
                timeout: None,
                on_error: None,
                on_timeout: None,
                shadow_root: None,
                iframe: None,
            },
        ];

        let flow_file = FlowFile { flows: HashMap::new() };

        run_actions(cdp_url, &actions, &mut context, false, &flow_file)
            .await
            .expect("Failed to run actions");

        let v1 = context.get("CODE").expect("CODE should be defined");
        assert_eq!(v1, &serde_yaml::Value::String("ABC".to_string()));
    }
}
