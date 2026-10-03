use thiserror::Error;
use crate::flow::Error as FlowError;

#[derive(Error, Debug)]
pub enum Error {
    #[error("YAML parse error: {0}")]
    Parse(#[from] serde_yaml::Error),

    #[error("Playwright error: {0}")]
    Playwright(String),

    #[error("Selector parse error: {0}")]
    SelectorParse(String),

    #[error("Variable not found: {0}")]
    VariableNotFound(String),

    #[error("Timeout waiting for: {0}")]
    Timeout(String),

    #[error("Action execution failed: {0}")]
    ActionFailed(String),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Ask action requires CLI runtime")]
    AskRequiresCli,

    #[error("Invalid action: {0}")]
    InvalidAction(String),
}

impl From<FlowError> for Error {
    fn from(e: FlowError) -> Self {
        match e {
            FlowError::VariableNotFound(v) => Error::VariableNotFound(v),
            FlowError::Parse(e) => Error::Parse(e),
            FlowError::Playwright(e) => Error::Playwright(e),
            FlowError::SelectorParse(e) => Error::SelectorParse(e),
            FlowError::Timeout(e) => Error::Timeout(e),
            FlowError::ActionFailed(e) => Error::ActionFailed(e),
            FlowError::ConnectionFailed(e) => Error::ConnectionFailed(e),
            FlowError::Io(e) => Error::Io(e),
            FlowError::AskRequiresCli => Error::AskRequiresCli,
            FlowError::InvalidAction(e) => Error::InvalidAction(e),
        }
    }
}
