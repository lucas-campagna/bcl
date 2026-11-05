use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct GotoOptions {
    pub wait_until: Option<String>,
    pub timeout: Option<u64>,
    pub referer: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct WaitOptions {
    pub state: Option<String>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ClickOptions {
    pub button: Option<String>,
    pub click_count: Option<u32>,
    pub delay: Option<u64>,
    pub timeout: Option<u64>,
    pub position: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default)]
pub struct DblClickOptions {
    pub button: Option<String>,
    pub delay: Option<u64>,
    pub timeout: Option<u64>,
    pub position: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default)]
pub struct HoverOptions {
    pub timeout: Option<u64>,
    pub position: Option<(f64, f64)>,
}

#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct FillOptions {
    pub force: Option<bool>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct FilterOptions;

#[derive(Debug, Clone, Default)]
pub struct SelectOptionOptions {
    pub force: Option<bool>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct SelectOptionValue {
    pub value: Option<String>,
    pub label: Option<String>,
    pub index: Option<u32>,
}

impl SelectOptionValue {
    pub fn by_value(s: impl Into<String>) -> Self {
        Self {
            value: Some(s.into()),
            ..Self::default()
        }
    }

    pub fn by_label(s: impl Into<String>) -> Self {
        Self {
            label: Some(s.into()),
            ..Self::default()
        }
    }

    pub fn by_index(i: u32) -> Self {
        Self {
            index: Some(i),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SetInputFilesOptions {
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum InputFiles {
    Paths(Vec<PathBuf>),
    Payloads(Vec<FilePayload>),
}

#[derive(Debug, Clone)]
pub struct FilePayload {
    pub name: String,
    pub mime_type: String,
    pub buffer: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct DragAndDropOptions {
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct PageCloseOptions {
    pub run_before_unload: Option<bool>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct KeyboardPressOptions {
    pub delay: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectOptions;

#[derive(Debug, Clone, Default)]
pub struct ConnectOverCdpOptions;
