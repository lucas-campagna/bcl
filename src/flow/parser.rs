use crate::error::Error;
use crate::flow::{Action, Condition, FlowFile};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub fn parse_file<P: AsRef<Path>>(path: P) -> Result<FlowFile, Error> {
    let content = fs::read_to_string(path)?;
    parse_str(&content)
}

pub fn parse_str(content: &str) -> Result<FlowFile, Error> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(content)?;
    parse_yaml_value(yaml)
}

fn parse_yaml_value(value: serde_yaml::Value) -> Result<FlowFile, Error> {
    let mut flows: HashMap<String, Vec<Action>> = HashMap::new();

    if let serde_yaml::Value::Mapping(map) = value {
        for (key, val) in map {
            let key_str = match key {
                serde_yaml::Value::String(s) => s,
                serde_yaml::Value::Number(n) => {
                    if let Some(i) = n.as_u64() {
                        format!("step_{}", i)
                    } else {
                        continue;
                    }
                }
                _ => continue,
            };

            if let serde_yaml::Value::Sequence(seq) = val {
                let mut actions = Vec::new();
                for item in seq {
                    if let serde_yaml::Value::Mapping(item_map) = item {
                        let action = parse_action_from_map(&item_map)?;
                        if let Some(action) = action {
                            actions.push(action);
                        }
                    }
                }
                if !actions.is_empty() {
                    flows.insert(key_str, actions);
                }
            }
        }
    }

    Ok(FlowFile { flows })
}

fn parse_condition(map: &serde_yaml::Mapping) -> Result<Option<Condition>, Error> {
    let mut condition = None;

    for (k, v) in map {
        if let serde_yaml::Value::String(key) = k {
            match key.as_str() {
                "xpath" if condition.is_none() => {
                    if let Ok(s) = parse_string_value(v) {
                        condition = Some(Condition::XPath(s));
                    }
                }
                "css" if condition.is_none() => {
                    if let Ok(s) = parse_string_value(v) {
                        condition = Some(Condition::Css(s));
                    }
                }
                "url" if condition.is_none() => {
                    if let Ok(s) = parse_string_value(v) {
                        condition = Some(Condition::Url(s));
                    }
                }
                "defined" if condition.is_none() => {
                    if let Ok(s) = parse_string_value(v) {
                        condition = Some(Condition::Defined(s));
                    }
                }
                "not" if condition.is_none() => {
                    if let serde_yaml::Value::Mapping(nested_map) = v {
                        if let Some(nested_cond) = parse_condition(nested_map)? {
                            condition = Some(Condition::Not(Box::new(nested_cond)));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(condition)
}

fn parse_action_from_map(map: &serde_yaml::Mapping) -> Result<Option<Action>, Error> {
    let mut action_keys: Vec<String> = Vec::new();
    let mut action_values: Vec<serde_yaml::Value> = Vec::new();

    for (k, v) in map {
        if let serde_yaml::Value::String(s) = k {
            action_keys.push(s.clone());
            action_values.push(v.clone());
        }
    }

    if action_keys.is_empty() {
        return Ok(None);
    }

    let first_key = &action_keys[0];
    let first_val = &action_values[0];

    match first_key.as_str() {
        "goto" => {
            let url = parse_string_value(first_val)?;
            let mut wait_until = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "wait_until" => wait_until = parse_string_value(val).ok(),
                    _ => {}
                }
            }
            Ok(Some(Action::Goto { url, wait_until }))
        }
        "type" => {
            let value = parse_string_value(first_val)?;
            let mut xpath = None;
            let mut css = None;
            let mut secret = false;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "xpath" => xpath = parse_string_value(val).ok(),
                    "css" => css = parse_string_value(val).ok(),
                    "secret" => secret = val.as_bool().unwrap_or(false),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Type { value, xpath, css, secret, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "click" => {
            let mut xpath = None;
            let mut css = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                if s.starts_with('/') || s.starts_with("//") {
                    xpath = Some(s.clone());
                } else {
                    css = Some(s.clone());
                }
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "xpath" => xpath = parse_string_value(val).ok(),
                    "css" => css = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Click { xpath, css, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "press" => {
            let key_name = parse_string_value(first_val)?;
            Ok(Some(Action::Press { key: key_name }))
        }
        "wait" => {
            let mut url = None;
            let mut xpath = None;
            let mut css = None;
            let mut duration = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                if s.starts_with('/') || s.starts_with("//") {
                    xpath = Some(s.clone());
                } else if s.contains("://") {
                    url = Some(s.clone());
                }
            } else if let serde_yaml::Value::Number(n) = first_val {
                if let Some(ms) = n.as_u64() {
                    duration = Some(ms);
                }
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "url" => url = parse_string_value(val).ok(),
                    "xpath" => xpath = parse_string_value(val).ok(),
                    "css" => css = parse_string_value(val).ok(),
                    "duration" => {
                        if let serde_yaml::Value::Number(n) = val {
                            duration = n.as_u64();
                        }
                    }
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Wait { url, xpath, css, duration, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "ask" => {
            let prompt = parse_string_value(first_val)?;
            let mut to = "TOTP".to_string();
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                if key.as_str() == "to" {
                    if let Ok(n) = parse_string_value(val) {
                        to = n;
                    }
                }
            }
            Ok(Some(Action::Ask { prompt, to }))
        }
        "hover" => {
            let mut xpath = None;
            let mut css = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                if s.starts_with('/') || s.starts_with("//") {
                    xpath = Some(s.clone());
                } else {
                    css = Some(s.clone());
                }
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "xpath" => xpath = parse_string_value(val).ok(),
                    "css" => css = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Hover { xpath, css, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "if" => {
            let mut condition = None;
            let mut then = Vec::new();
            let mut else_ = Vec::new();

            if let serde_yaml::Value::Mapping(inner_map) = first_val {
                condition = parse_condition(inner_map)?;
            }

            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "then" => {
                        if let serde_yaml::Value::Sequence(seq) = val {
                            for item in seq {
                                if let serde_yaml::Value::Mapping(item_map) = item {
                                    let action = parse_action_from_map(item_map)?;
                                    if let Some(a) = action {
                                        then.push(a);
                                    }
                                }
                            }
                        }
                    }
                    "else" | "else_" => {
                        if let serde_yaml::Value::Sequence(seq) = val {
                            for item in seq {
                                if let serde_yaml::Value::Mapping(item_map) = item {
                                    let action = parse_action_from_map(item_map)?;
                                    if let Some(a) = action {
                                        else_.push(a);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            if let Some(c) = condition {
                Ok(Some(Action::If { condition: c, then, else_ }))
            } else {
                Err(Error::InvalidAction("If action requires a condition".to_string()))
            }
        }
        "read" => {
            let mut xpath = None;
            let mut css = None;
            if let serde_yaml::Value::String(s) = first_val {
                if s.starts_with('/') || s.starts_with("//") {
                    xpath = Some(s.clone());
                } else {
                    css = Some(s.clone());
                }
            }
            let mut to = None;
            let mut html = false;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "xpath" => xpath = parse_string_value(val).ok(),
                    "css" => css = parse_string_value(val).ok(),
                    "to" => to = parse_string_value(val).ok(),
                    "html" => html = parse_bool_value(val).unwrap_or(false),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Read { xpath, css, to, html, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "log" => {
            let text = parse_string_value(first_val)?;
            Ok(Some(Action::Log { text }))
        }
        "call" => {
            let flow_name = parse_string_value(first_val)?;
            let mut params = std::collections::HashMap::new();
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                if key.as_str() != "flow" {
                    params.insert(key.clone(), val.clone());
                }
            }
            Ok(Some(Action::Call { flow: flow_name, params }))
        }
        "js" => {
            let code = match first_val {
                serde_yaml::Value::String(s) => s.clone(),
                serde_yaml::Value::Number(n) => n.to_string(),
                _ => return Err(Error::InvalidAction("Expected string or number value".to_string())),
            };
            let mut to = None;
            let mut on_error = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "to" => to = parse_string_value(val).ok(),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    _ => {}
                }
            }
            Ok(Some(Action::Js { code, to, on_error }))
        }
        "define" | "set" => {
            let var = parse_string_value(first_val)?;
            let mut value = serde_yaml::Value::Null;
            let mut overwrite = true;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "value" => value = val.clone(),
                    "overwrite" => overwrite = val.as_bool().unwrap_or(true),
                    _ => {}
                }
            }
            Ok(Some(Action::Define { var, value, overwrite }))
        }
        _ => Err(Error::InvalidAction(format!("Unknown action: {}", first_key))),
    }
}

fn parse_string_value(value: &serde_yaml::Value) -> Result<String, Error> {
    match value {
        serde_yaml::Value::String(s) => Ok(s.clone()),
        _ => Err(Error::InvalidAction("Expected string value".to_string())),
    }
}

fn parse_bool_value(value: &serde_yaml::Value) -> Result<bool, Error> {
    match value {
        serde_yaml::Value::Bool(b) => Ok(*b),
        serde_yaml::Value::String(s) => s.parse().map_err(|_| Error::InvalidAction("Expected bool value".to_string())),
        _ => Err(Error::InvalidAction("Expected bool value".to_string())),
    }
}

fn parse_timeout_value(value: &serde_yaml::Value) -> Option<u64> {
    match value {
        serde_yaml::Value::Number(n) => n.as_u64(),
        serde_yaml::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn parse_actions_from_value(value: &serde_yaml::Value) -> Vec<Action> {
    let mut actions = Vec::new();
    if let serde_yaml::Value::Sequence(seq) = value {
        for item in seq {
            if let serde_yaml::Value::Mapping(item_map) = item {
                if let Ok(Some(action)) = parse_action_from_map(&item_map) {
                    actions.push(action);
                }
            }
        }
    }
    actions
}

fn parse_selector_path_value(value: &serde_yaml::Value) -> Option<crate::flow::SelectorPath> {
    match value {
        serde_yaml::Value::String(s) => Some(crate::flow::SelectorPath::Single(s.clone())),
        serde_yaml::Value::Sequence(seq) => {
            let paths: Vec<String> = seq.iter()
                .filter_map(|v| {
                    if let serde_yaml::Value::String(s) = v {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if paths.is_empty() {
                None
            } else {
                Some(crate::flow::SelectorPath::Multiple(paths))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_open_action() {
        let yaml = r#"
# flow name
auth:
  - goto: https://example.com
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("auth").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Goto { url, wait_until } => {
                assert_eq!(url, "https://example.com");
                assert!(wait_until.is_none());
            }
            _ => panic!("Expected Goto action"),
        }
    }

    #[test]
    fn test_parse_type_with_xpath() {
        let yaml = r#"
# flow name
auth:
  - type: $CPF
    xpath: //*[@id="input"]
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("auth").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Type { value, xpath, css, .. } => {
                assert_eq!(value, "$CPF");
                assert_eq!(xpath.as_ref().unwrap(), "//*[@id=\"input\"]");
                assert!(css.is_none());
            }
            _ => panic!("Expected Type action"),
        }
    }

    #[test]
    fn test_parse_type_with_secret() {
        let yaml = r#"
# flow name
auth:
  - type: $PASSWORD
    xpath: //*[@id="pwd"]
    secret: true
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("auth").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Type { value, xpath, css: _, secret, .. } => {
                assert_eq!(value, "$PASSWORD");
                assert_eq!(xpath.as_ref().unwrap(), "//*[@id=\"pwd\"]");
                assert!(*secret);
            }
            _ => panic!("Expected Type action"),
        }
    }

    #[test]
    fn test_parse_define_with_string_value() {
        let yaml = r#"
test:
  - define: A
    value: hello
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Define { var, value, overwrite } => {
                assert_eq!(var, "A");
                assert_eq!(value, &serde_yaml::Value::String("hello".to_string()));
                assert!(*overwrite);
            }
            _ => panic!("Expected Define action"),
        }
    }

    #[test]
    fn test_parse_define_with_number_value() {
        let yaml = r#"
test:
  - define: A
    value: 123
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Define { var, value, .. } => {
                assert_eq!(var, "A");
                assert_eq!(value, &serde_yaml::Value::Number(123.into()));
            }
            _ => panic!("Expected Define action"),
        }
    }

    #[test]
    fn test_parse_define_with_bool_value() {
        let yaml = r#"
test:
  - define: A
    value: true
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Define { var, value, .. } => {
                assert_eq!(var, "A");
                assert_eq!(value, &serde_yaml::Value::Bool(true));
            }
            _ => panic!("Expected Define action"),
        }
    }

    #[test]
    fn test_parse_define_with_object_value() {
        let yaml = r#"
test:
  - define: A
    value: {key: value}
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Define { var, value, .. } => {
                assert_eq!(var, "A");
                match value {
                    serde_yaml::Value::Mapping(map) => {
                        assert_eq!(map.len(), 1);
                    }
                    _ => panic!("Expected Mapping"),
                }
            }
            _ => panic!("Expected Define action"),
        }
    }

    #[test]
    fn test_parse_define_with_overwrite_false() {
        let yaml = r#"
test:
  - define: A
    value: hello
    overwrite: false
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Define { var, value, overwrite } => {
                assert_eq!(var, "A");
                assert_eq!(value, &serde_yaml::Value::String("hello".to_string()));
                assert!(!*overwrite);
            }
            _ => panic!("Expected Define action"),
        }
    }

    #[test]
    fn test_parse_define_set_alias() {
        let yaml = r#"
test:
  - set: A
    value: 123
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Define { var, .. } => {
                assert_eq!(var, "A");
            }
            _ => panic!("Expected Define action"),
        }
    }

    #[test]
    fn test_parse_if_defined_condition() {
        let yaml = r#"
test:
  - if: {defined: $TOKEN}
    then:
      - log: found
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::If { condition, then, else_ } => {
                match condition {
                    crate::flow::Condition::Defined(var) => {
                        assert_eq!(var, "$TOKEN");
                    }
                    _ => panic!("Expected Defined condition"),
                }
                assert_eq!(then.len(), 1);
                assert!(else_.is_empty());
            }
            _ => panic!("Expected If action"),
        }
    }

    #[test]
    fn test_parse_if_not_defined_condition() {
        let yaml = r#"
test:
  - if: {not: {defined: $TOKEN}}
    then:
      - log: not found
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::If { condition, then, else_ } => {
                match condition {
                    crate::flow::Condition::Not(inner) => {
                        match inner.as_ref() {
                            crate::flow::Condition::Defined(var) => {
                                assert_eq!(var, "$TOKEN");
                            }
                            _ => panic!("Expected Defined condition inside Not"),
                        }
                    }
                    _ => panic!("Expected Not condition"),
                }
                assert_eq!(then.len(), 1);
                assert!(else_.is_empty());
            }
            _ => panic!("Expected If action"),
        }
    }

    #[test]
    fn test_parse_read_with_to() {
        let yaml = r#"
test:
  - read: //*[@id="title"]
    to: TITLE
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Read { xpath, css, to, html, .. } => {
                assert_eq!(xpath.as_ref().unwrap(), "//*[@id=\"title\"]");
                assert!(css.is_none());
                assert_eq!(to.as_ref().unwrap(), "TITLE");
                assert!(!*html);
            }
            _ => panic!("Expected Read action"),
        }
    }

    #[test]
    fn test_parse_read_without_to() {
        let yaml = r#"
test:
  - read: //*[@id="title"]
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Read { xpath, css, to, html, .. } => {
                assert_eq!(xpath.as_ref().unwrap(), "//*[@id=\"title\"]");
                assert!(css.is_none());
                assert!(to.is_none());
                assert!(!*html);
            }
            _ => panic!("Expected Read action"),
        }
    }

    #[test]
    fn test_parse_read_with_html() {
        let yaml = r#"
test:
  - read: //*[@id="content"]
    to: CONTENT
    html: true
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Read { xpath, css, to, html, .. } => {
                assert_eq!(xpath.as_ref().unwrap(), "//*[@id=\"content\"]");
                assert!(css.is_none());
                assert_eq!(to.as_ref().unwrap(), "CONTENT");
                assert!(*html);
            }
            _ => panic!("Expected Read action"),
        }
    }

    #[test]
    fn test_parse_read_with_css_and_html_false() {
        let yaml = r#"
test:
  - read: .message
    to: MSG
    html: false
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Read { xpath, css, to, html, .. } => {
                assert!(xpath.is_none());
                assert_eq!(css.as_ref().unwrap(), ".message");
                assert_eq!(to.as_ref().unwrap(), "MSG");
                assert!(!*html);
            }
            _ => panic!("Expected Read action"),
        }
    }

    #[test]
    fn test_parse_click_with_shadow_root_and_iframe() {
        let yaml = r##"
test:
  - click: "#button"
    shadow_root: "#inner-shadow"
    iframe: "#frame1"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Click { xpath, css, shadow_root, iframe, .. } => {
                assert!(xpath.is_none());
                assert_eq!(css.as_ref().unwrap(), "#button");
                assert_eq!(shadow_root.as_ref().unwrap(), &crate::flow::SelectorPath::Single("#inner-shadow".to_string()));
                assert_eq!(iframe.as_ref().unwrap(), &crate::flow::SelectorPath::Single("#frame1".to_string()));
            }
            _ => panic!("Expected Click action"),
        }
    }

    #[test]
    fn test_parse_type_with_nested_shadow_root() {
        let yaml = r##"
test:
  - type: "hello world"
    xpath: "//input[@id='search']"
    shadow_root:
      - "#outer"
      - "#inner"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Type { xpath, shadow_root, .. } => {
                assert_eq!(xpath.as_ref().unwrap(), "//input[@id='search']");
                assert_eq!(
                    shadow_root.as_ref().unwrap(),
                    &crate::flow::SelectorPath::Multiple(vec![
                        "#outer".to_string(),
                        "#inner".to_string()
                    ])
                );
            }
            _ => panic!("Expected Type action"),
        }
    }

    #[test]
    fn test_parse_wait_with_shadow_root_single() {
        let yaml = r##"
test:
  - wait: "//div[@class='loading']"
    shadow_root: "#shadow-container"
    duration: 500
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Wait { url, xpath, css, shadow_root, duration, .. } => {
                assert!(url.is_none());
                assert!(css.is_none());
                assert_eq!(xpath.as_ref().unwrap(), "//div[@class='loading']");
                assert_eq!(shadow_root.as_ref().unwrap(), &crate::flow::SelectorPath::Single("#shadow-container".to_string()));
                assert_eq!(*duration, Some(500));
            }
            _ => panic!("Expected Wait action"),
        }
    }
}
