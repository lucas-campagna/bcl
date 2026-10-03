use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod parser;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    pub name: Option<String>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone)]
pub struct FlowFile {
    pub flows: HashMap<String, Vec<Action>>,
}

impl FlowFile {
    pub fn get(&self, name: &str) -> Option<&Vec<Action>> {
        self.flows.get(name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SelectorPath {
    Single(String),
    Multiple(Vec<String>),
}

impl Default for SelectorPath {
    fn default() -> Self {
        SelectorPath::Single(String::new())
    }
}

impl SelectorPath {
    pub fn as_slice(&self) -> &[String] {
        match self {
            SelectorPath::Single(s) => std::slice::from_ref(s),
            SelectorPath::Multiple(v) => v,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Action {
    Goto { url: String, wait_until: Option<String> },
    Type {
        value: String,
        target: Option<String>,
        #[serde(default)] secret: bool,
        #[serde(default)] timeout: Option<u64>,
        #[serde(default)] on_error: Option<Vec<Action>>,
        #[serde(default)] on_timeout: Option<Vec<Action>>,
        #[serde(default)] shadow_root: Option<SelectorPath>,
        #[serde(default)] iframe: Option<SelectorPath>,
    },
    Click {
        target: Option<String>,
        #[serde(default)] timeout: Option<u64>,
        #[serde(default)] on_error: Option<Vec<Action>>,
        #[serde(default)] on_timeout: Option<Vec<Action>>,
        #[serde(default)] shadow_root: Option<SelectorPath>,
        #[serde(default)] iframe: Option<SelectorPath>,
    },
    Press { key: String },
    Wait {
        url: Option<String>,
        target: Option<String>,
        duration: Option<u64>,
        #[serde(default)] timeout: Option<u64>,
        #[serde(default)] on_error: Option<Vec<Action>>,
        #[serde(default)] on_timeout: Option<Vec<Action>>,
        #[serde(default)] shadow_root: Option<SelectorPath>,
        #[serde(default)] iframe: Option<SelectorPath>,
    },
    Ask { prompt: String, to: String },
    Hover {
        target: Option<String>,
        #[serde(default)] timeout: Option<u64>,
        #[serde(default)] on_error: Option<Vec<Action>>,
        #[serde(default)] on_timeout: Option<Vec<Action>>,
        #[serde(default)] shadow_root: Option<SelectorPath>,
        #[serde(default)] iframe: Option<SelectorPath>,
    },
    If {
        #[serde(flatten)]
        condition: Condition,
        #[serde(default)]
        then: Vec<Action>,
        #[serde(default)]
        else_: Vec<Action>,
    },
    Read {
        target: Option<String>,
        to: Option<String>,
        #[serde(default)] html: bool,
        #[serde(default)] timeout: Option<u64>,
        #[serde(default)] on_error: Option<Vec<Action>>,
        #[serde(default)] on_timeout: Option<Vec<Action>>,
        #[serde(default)] shadow_root: Option<SelectorPath>,
        #[serde(default)] iframe: Option<SelectorPath>,
    },
    Log { text: String },
    Call { flow: String, #[serde(default)] params: std::collections::HashMap<String, serde_yaml::Value> },
    Js { code: String, to: Option<String>, #[serde(default)] on_error: Option<Vec<Action>> },
    Define { var: String, value: serde_yaml::Value, #[serde(default = "default_true")] overwrite: bool },
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Condition {
    XPath(String),
    Css(String),
    Url(String),
    Defined(String),
    Not(Box<Condition>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    XPath(String),
    Css(String),
}

impl Selector {
    pub fn new(raw: impl Into<String>) -> Self {
        let s = raw.into();
        if s.starts_with('/') || s.starts_with("//") {
            Selector::XPath(s)
        } else if s.contains('#') || s.contains('.') || s.contains('[') || s.contains(' ') {
            Selector::Css(s)
        } else {
            Selector::XPath(s)
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Selector::XPath(s) => s,
            Selector::Css(s) => s,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Context {
    vars: HashMap<String, serde_yaml::Value>,
}

enum YamlResolveResult {
    Found(serde_yaml::Value),
    NotFound,
    Empty,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(key.into(), serde_yaml::Value::String(value.into()));
    }

    pub fn set_yaml(&mut self, key: impl Into<String>, value: serde_yaml::Value) {
        self.vars.insert(key.into(), value);
    }

    pub fn get(&self, key: &str) -> Option<&serde_yaml::Value> {
        self.vars.get(key)
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.vars.get(key).map(|v| serde_yaml::to_string(v).unwrap_or_default())
    }

    pub fn is_defined(&self, key: &str) -> bool {
        let base_var_regex = regex::Regex::new(r"^([A-Z_][A-Z0-9_]*)").unwrap();
        if let Some(caps) = base_var_regex.captures(key) {
            if let Some(name) = caps.get(1) {
                return self.vars.contains_key(name.as_str());
            }
        }
        false
    }

    fn normalize_yaml_syntax(s: &str) -> String {
        let s = s.to_string();
        let mut result = String::with_capacity(s.len() * 2);
        let mut in_flow_mapping = false;
        let mut chars = s.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' {
                in_flow_mapping = true;
                result.push(c);
            } else if c == '}' {
                in_flow_mapping = false;
                result.push(c);
            } else if c == ':' && in_flow_mapping {
                let next = chars.peek();
                if let Some(&next_c) = next {
                    if !next_c.is_whitespace() && next_c != ',' && next_c != '}' && next_c != ']' {
                        result.push_str(": ");
                        continue;
                    }
                }
                result.push(c);
            } else {
                result.push(c);
            }
        }
        result
    }

    pub fn try_parse_yaml(s: &str) -> serde_yaml::Value {
        let normalized = Self::normalize_yaml_syntax(s);
        if let Ok(v) = serde_yaml::from_str(&normalized) {
            v
        } else if let Ok(v) = serde_yaml::from_str(s) {
            v
        } else {
            serde_yaml::Value::String(s.to_string())
        }
    }

    fn get_yaml_value(&self, var_name: &str) -> YamlResolveResult {
        let path_regex = regex::Regex::new(r"\.([a-zA-Z_][a-zA-Z0-9_]*)|\[([0-9]+)\]").unwrap();

        let base_var_regex = regex::Regex::new(r"^([A-Z_][A-Z0-9_]*)").unwrap();
        let base_name = match base_var_regex.captures(var_name).and_then(|c| c.get(1)).map(|m| m.as_str()) {
            Some(name) => name,
            None => return YamlResolveResult::NotFound,
        };

        let current = match self.get(base_name) {
            Some(v) => v.clone(),
            None => return YamlResolveResult::NotFound,
        };

        let mut current = current;

        for cap in path_regex.captures_iter(var_name) {
            if let (Some(key), None) = (cap.get(1), cap.get(2)) {
                match current {
                    serde_yaml::Value::Mapping(map) => {
                        if let Some(v) = map.get(&serde_yaml::Value::String(key.as_str().to_string())) {
                            current = v.clone();
                        } else {
                            return YamlResolveResult::Empty;
                        }
                    }
                    _ => return YamlResolveResult::Empty,
                }
            } else if let (None, Some(idx)) = (cap.get(1), cap.get(2)) {
                let index: usize = match idx.as_str().parse() {
                    Ok(i) => i,
                    Err(_) => return YamlResolveResult::Empty,
                };
                match current {
                    serde_yaml::Value::Sequence(seq) => {
                        if index < seq.len() {
                            current = seq[index].clone();
                        } else {
                            return YamlResolveResult::Empty;
                        }
                    }
                    _ => return YamlResolveResult::Empty,
                }
            }
        }
        YamlResolveResult::Found(current)
    }

    pub fn resolve(&self, input: &str) -> String {
        self.resolve_with_url(input, None)
    }

    pub fn resolve_with_url(&self, input: &str, current_url: Option<&str>) -> String {
        let var_regex = regex::Regex::new(r"\$([A-Z_][A-Z0-9_\.a-zA-Z\[\]0-9]*|url)").unwrap();
        let mut result = input.to_string();
        for cap in var_regex.captures_iter(input) {
            let full_match = &cap[0];
            let var_name = &cap[1];

            if var_name == "url" {
                if let Some(url) = current_url {
                    result = result.replace(full_match, url);
                }
                continue;
            }

            let value = match self.get_yaml_value(var_name) {
                YamlResolveResult::Found(v) => {
                    match v {
                        serde_yaml::Value::String(s) => s,
                        _ => serde_yaml::to_string(&v).unwrap_or_default().trim().to_string(),
                    }
                }
                YamlResolveResult::Empty => String::new(),
                YamlResolveResult::NotFound => format!("${}", var_name),
            };
            result = result.replace(full_match, &value);
        }
        result
    }

    pub fn resolve_value(&self, input: &str) -> Result<String, Error> {
        Ok(self.resolve(input))
    }

    pub fn resolve_value_with_url(&self, input: &str, current_url: Option<&str>) -> Result<String, Error> {
        Ok(self.resolve_with_url(input, current_url))
    }
}

use thiserror::Error;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selector_detection_xpath() {
        let s = Selector::new("//*[@id='foo']/div");
        assert!(matches!(s, Selector::XPath(_)));
    }

    #[test]
    fn test_selector_detection_css() {
        let s = Selector::new("#foo > div.bar");
        assert!(matches!(s, Selector::Css(_)));
    }

    #[test]
    fn test_context_resolve() {
        let mut ctx = Context::new();
        ctx.set("CPF", "12345678900");
        assert_eq!(ctx.resolve("$CPF"), "12345678900");
        assert_eq!(ctx.resolve("user:$CPF"), "user:12345678900");
    }

    #[test]
    fn test_context_resolve_dot_notation() {
        let mut ctx = Context::new();
        let mut map = serde_yaml::Mapping::new();
        map.insert(serde_yaml::Value::String("x".to_string()), serde_yaml::Value::Number(123.into()));
        ctx.set_yaml("ABC", serde_yaml::Value::Mapping(map));
        assert_eq!(ctx.resolve("$ABC.x"), "123");
    }

    #[test]
    fn test_context_resolve_yaml_from_env() {
        let yaml_str = "{a:123}";
        let yaml_val = Context::try_parse_yaml(yaml_str);
        let mut ctx = Context::new();
        ctx.set_yaml("ABC", yaml_val);
        let resolved = ctx.resolve("$ABC");
        assert_eq!(resolved, "a: 123");
    }

    #[test]
    fn test_resolve_nested_array_index() {
        let yaml_str = "{x:[1,2,3]}";
        let yaml_val = Context::try_parse_yaml(yaml_str);
        let mut ctx = Context::new();
        ctx.set_yaml("ABC", yaml_val);
        assert_eq!(ctx.resolve("$ABC.x[0]"), "1");
    }
}
