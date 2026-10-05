use crate::error::Error;
use crate::flow::{Action, Condition, FlowFile};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;

pub fn parse_file<P: AsRef<Path>>(path: P) -> Result<FlowFile, Error> {
    let content = fs::read_to_string(path)?;
    parse_str(&content)
}

pub fn parse_stdin() -> Result<FlowFile, Error> {
    let mut content = String::new();
    std::io::stdin().read_to_string(&mut content).map_err(|e| Error::InvalidInput(format!("Failed to read stdin: {}", e)))?;
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
                "text" if condition.is_none() => {
                    if let serde_yaml::Value::Mapping(text_map) = v {
                        let mut target = None;
                        let mut contains = None;
                        for (tk, tv) in text_map {
                            if let serde_yaml::Value::String(tk_str) = tk {
                                match tk_str.as_str() {
                                    "target" => target = parse_string_value(tv).ok(),
                                    "contains" => contains = parse_string_value(tv).ok(),
                                    _ => {}
                                }
                            }
                        }
                        if let (Some(t), Some(c)) = (target, contains) {
                            condition = Some(Condition::Text { target: t, contains: c });
                        }
                    }
                }
                "visible" | "checked" | "enabled" if condition.is_none() => {
                    let selector = match v {
                        serde_yaml::Value::String(s) => Some(s.clone()),
                        serde_yaml::Value::Mapping(sel_map) => {
                            let mut sel = None;
                            for (tk, tv) in sel_map {
                                if let serde_yaml::Value::String(tk_str) = tk {
                                    if tk_str == "target" || tk_str == "css" || tk_str == "xpath" {
                                        sel = parse_string_value(tv).ok();
                                    }
                                }
                            }
                            sel
                        }
                        _ => None,
                    };
                    let cond_name = key.as_str();
                    condition = Some(match cond_name {
                        "visible" => Condition::Visible(selector),
                        "checked" => Condition::Checked(selector),
                        "enabled" => Condition::Enabled(selector),
                        _ => unreachable!(),
                    });
                }
                "equals" if condition.is_none() => {
                    if let serde_yaml::Value::Mapping(eq_map) = v {
                        let mut var = None;
                        let mut value = None;
                        for (tk, tv) in eq_map {
                            if let serde_yaml::Value::String(tk_str) = tk {
                                match tk_str.as_str() {
                                    "var" => var = parse_string_value(tv).ok(),
                                    "value" => value = parse_string_value(tv).ok(),
                                    _ => {}
                                }
                            }
                        }
                        if let (Some(vn), Some(vl)) = (var, value) {
                            condition = Some(Condition::Equals { var: vn, value: vl });
                        }
                    }
                }
                "and" | "or" if condition.is_none() => {
                    if let serde_yaml::Value::Sequence(items) = v {
                        let mut conditions = Vec::new();
                        for item in items {
                            if let serde_yaml::Value::Mapping(item_map) = item {
                                if let Some(c) = parse_condition(item_map)? {
                                    conditions.push(c);
                                }
                            }
                        }
                        if !conditions.is_empty() {
                            condition = Some(if key == "and" {
                                Condition::And(conditions)
                            } else {
                                Condition::Or(conditions)
                            });
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
            let mut target = None;
            let mut secret = false;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "secret" => secret = val.as_bool().unwrap_or(false),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Type { value, target, secret, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "click" => {
            let mut target = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                target = Some(s.clone());
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Click { target, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "press" => {
            let key_name = parse_string_value(first_val)?;
            Ok(Some(Action::Press { key: key_name }))
        }
        "wait" => {
            let mut url = None;
            let mut target = None;
            let mut duration = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                if s.contains("://") {
                    url = Some(s.clone());
                } else {
                    target = Some(s.clone());
                }
            } else if let serde_yaml::Value::Number(n) = first_val {
                if let Some(ms) = n.as_u64() {
                    duration = Some(ms);
                }
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "url" => url = parse_string_value(val).ok(),
                    "target" => target = parse_string_value(val).ok(),
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
            Ok(Some(Action::Wait { url, target, duration, timeout, on_error, on_timeout, shadow_root, iframe }))
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
            let mut target = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                target = Some(s.clone());
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Hover { target, timeout, on_error, on_timeout, shadow_root, iframe }))
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
            let mut target = None;
            if let serde_yaml::Value::String(s) = first_val {
                target = Some(s.clone());
            }
            let mut to = None;
            let mut html = false;
            let mut attribute = None;
            let mut value = false;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "to" => to = parse_string_value(val).ok(),
                    "html" => html = parse_bool_value(val).unwrap_or(false),
                    "attribute" => attribute = parse_string_value(val).ok(),
                    "value" => value = parse_bool_value(val).unwrap_or(false),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Read { target, to, html, attribute, value, timeout, on_error, on_timeout, shadow_root, iframe }))
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
        "select" => {
            let value = parse_string_value(first_val)?;
            let mut target = None;
            let mut values = Vec::new();
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "values" => {
                        if let serde_yaml::Value::Sequence(seq) = val {
                            values = seq.iter().filter_map(|v| parse_string_value(v).ok()).collect();
                        } else if let Ok(s) = parse_string_value(val) {
                            values = vec![s];
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
            Ok(Some(Action::Select { value, target, values, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "check" => {
            let mut target = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                target = Some(s.clone());
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Check { target, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "uncheck" => {
            let mut target = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                target = Some(s.clone());
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Uncheck { target, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "dblclick" => {
            let mut target = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                target = Some(s.clone());
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::DblClick { target, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "right_click" => {
            let mut target = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                target = Some(s.clone());
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::RightClick { target, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "clear" => {
            let mut target = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            if let serde_yaml::Value::String(s) = first_val {
                target = Some(s.clone());
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Clear { target, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "upload" => {
            let mut files = Vec::new();
            let mut target = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            match first_val {
                serde_yaml::Value::String(s) => files = vec![s.clone()],
                serde_yaml::Value::Sequence(seq) => {
                    files = seq.iter().filter_map(|v| parse_string_value(v).ok()).collect();
                }
                _ => {}
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "target" => target = parse_string_value(val).ok(),
                    "files" => {
                        if let serde_yaml::Value::Sequence(seq) = val {
                            files = seq.iter().filter_map(|v| parse_string_value(v).ok()).collect();
                        } else if let Ok(s) = parse_string_value(val) {
                            files = vec![s];
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
            Ok(Some(Action::Upload { files, target, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "drag" => {
            let source = parse_string_value(first_val)?;
            let mut to = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "to" => to = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    "on_error" => on_error = Some(parse_actions_from_value(val)),
                    "on_timeout" => on_timeout = Some(parse_actions_from_value(val)),
                    "shadow_root" => shadow_root = parse_selector_path_value(val),
                    "iframe" => iframe = parse_selector_path_value(val),
                    _ => {}
                }
            }
            let to = to.unwrap_or_default();
            Ok(Some(Action::Drag { source, to, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "scroll" => {
            let target = parse_string_value(first_val)?;
            let mut x = None;
            let mut y = None;
            let mut timeout = None;
            let mut on_error = None;
            let mut on_timeout = None;
            let mut shadow_root = None;
            let mut iframe = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "x" => {
                        if let serde_yaml::Value::Number(n) = val {
                            x = n.as_i64();
                        }
                    }
                    "y" => {
                        if let serde_yaml::Value::Number(n) = val {
                            y = n.as_i64();
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
            Ok(Some(Action::Scroll { target, x, y, timeout, on_error, on_timeout, shadow_root, iframe }))
        }
        "dialog" => {
            let mode = parse_string_value(first_val)?;
            let mut prompt_text = None;
            let mut timeout = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "prompt_text" => prompt_text = parse_string_value(val).ok(),
                    "timeout" => timeout = parse_timeout_value(val),
                    _ => {}
                }
            }
            Ok(Some(Action::Dialog { mode, prompt_text, timeout }))
        }
        "download" => {
            let path = parse_string_value(first_val)?;
            let mut timeout = None;
            let mut to = None;
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "timeout" => timeout = parse_timeout_value(val),
                    "to" => to = parse_string_value(val).ok(),
                    _ => {}
                }
            }
            Ok(Some(Action::Download { path, timeout, to }))
        }
        "for" => {
            let items = first_val.clone();
            let mut as_ = "ITEM".to_string();
            let mut do_ = Vec::new();
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "as" => {
                        if let Ok(s) = parse_string_value(val) {
                            as_ = s;
                        }
                    }
                    "do" => {
                        if let serde_yaml::Value::Sequence(seq) = val {
                            for item in seq {
                                if let serde_yaml::Value::Mapping(item_map) = item {
                                    if let Some(a) = parse_action_from_map(item_map)? {
                                        do_.push(a);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Some(Action::For { items, as_, do_ }))
        }
        "while" => {
            let mut condition = None;
            let mut do_ = Vec::new();
            let mut max = None;
            if let serde_yaml::Value::Mapping(while_map) = first_val {
                if let Some(cond_val) = while_map.get(&serde_yaml::Value::String("condition".to_string())) {
                    if let serde_yaml::Value::Mapping(cond_map) = cond_val {
                        condition = parse_condition(cond_map)?;
                    }
                }
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "condition" => {
                        if condition.is_none() {
                            if let serde_yaml::Value::Mapping(cond_map) = val {
                                condition = parse_condition(cond_map)?;
                            }
                        }
                    }
                    "xpath" | "css" | "url" | "defined" | "not" | "text" | "visible" | "checked" | "enabled" | "equals" | "and" | "or" => {
                        if condition.is_none() {
                            if let serde_yaml::Value::Mapping(cond_map) = val {
                                condition = parse_condition(cond_map)?;
                            }
                        }
                    }
                    "do" => {
                        if let serde_yaml::Value::Sequence(seq) = val {
                            for item in seq {
                                if let serde_yaml::Value::Mapping(item_map) = item {
                                    if let Some(a) = parse_action_from_map(item_map)? {
                                        do_.push(a);
                                    }
                                }
                            }
                        }
                    }
                    "max" => {
                        if let serde_yaml::Value::Number(n) = val {
                            max = n.as_u64();
                        }
                    }
                    _ => {}
                }
            }
            if let Some(c) = condition {
                Ok(Some(Action::While { condition: c, do_, max }))
            } else {
                Err(Error::InvalidAction("while action requires a condition".to_string()))
            }
        }
        "retry" => {
            let attempts = match first_val {
                serde_yaml::Value::Number(n) => n.as_u64().unwrap_or(1),
                _ => 1,
            };
            let mut interval = None;
            let mut do_ = Vec::new();
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "interval" => {
                        if let serde_yaml::Value::Number(n) = val {
                            interval = n.as_u64();
                        }
                    }
                    "do" => {
                        if let serde_yaml::Value::Sequence(seq) = val {
                            for item in seq {
                                if let serde_yaml::Value::Mapping(item_map) = item {
                                    if let Some(a) = parse_action_from_map(item_map)? {
                                        do_.push(a);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Some(Action::Retry { attempts, interval, do_ }))
        }
        "assert" => {
            let mut condition = None;
            let mut message = None;
            if let serde_yaml::Value::Mapping(assert_map) = first_val {
                if let Some(cond_val) = assert_map.get(&serde_yaml::Value::String("condition".to_string())) {
                    if let serde_yaml::Value::Mapping(cond_map) = cond_val {
                        condition = parse_condition(cond_map)?;
                    }
                }
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                match key.as_str() {
                    "condition" => {
                        if condition.is_none() {
                            if let serde_yaml::Value::Mapping(cond_map) = val {
                                condition = parse_condition(cond_map)?;
                            }
                        }
                    }
                    "xpath" | "css" | "url" | "defined" | "not" | "text" | "visible" | "checked" | "enabled" | "equals" | "and" | "or" => {
                        if condition.is_none() {
                            if let serde_yaml::Value::Mapping(cond_map) = val {
                                condition = parse_condition(cond_map)?;
                            }
                        }
                    }
                    "message" => message = parse_string_value(val).ok(),
                    _ => {}
                }
            }
            if let Some(c) = condition {
                Ok(Some(Action::Assert { condition: c, message }))
            } else {
                Err(Error::InvalidAction("assert action requires a condition".to_string()))
            }
        }
        "return" => {
            let mut values = std::collections::HashMap::new();
            if let serde_yaml::Value::Mapping(map) = first_val {
                for (k, v) in map {
                    if let serde_yaml::Value::String(key) = k {
                        values.insert(key.clone(), v.clone());
                    }
                }
            }
            for (key, val) in action_keys.iter().zip(action_values.iter()).skip(1) {
                if key.as_str() != "return" {
                    values.insert(key.clone(), val.clone());
                }
            }
            Ok(Some(Action::Return { values }))
        }
        "fail" => {
            let message = parse_string_value(first_val)?;
            Ok(Some(Action::Fail { message }))
        }
        "save" => {
            let path = parse_string_value(first_val)?;
            Ok(Some(Action::Save { path }))
        }
        "load" => {
            let path = parse_string_value(first_val)?;
            Ok(Some(Action::Load { path }))
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
    fn test_parse_type_with_target() {
        let yaml = r#"
# flow name
auth:
  - type: $CPF
    target: //*[@id="input"]
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("auth").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Type { value, target, .. } => {
                assert_eq!(value, "$CPF");
                assert_eq!(target.as_ref().unwrap(), "//*[@id=\"input\"]");
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
    target: //*[@id="pwd"]
    secret: true
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("auth").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Type { value, target, secret, .. } => {
                assert_eq!(value, "$PASSWORD");
                assert_eq!(target.as_ref().unwrap(), "//*[@id=\"pwd\"]");
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
            Action::Read { target, to, html, .. } => {
                assert_eq!(target.as_ref().unwrap(), "//*[@id=\"title\"]");
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
            Action::Read { target, to, html, .. } => {
                assert_eq!(target.as_ref().unwrap(), "//*[@id=\"title\"]");
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
            Action::Read { target, to, html, .. } => {
                assert_eq!(target.as_ref().unwrap(), "//*[@id=\"content\"]");
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
            Action::Read { target, to, html, .. } => {
                assert_eq!(target.as_ref().unwrap(), ".message");
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
            Action::Click { target, shadow_root, iframe, .. } => {
                assert_eq!(target.as_ref().unwrap(), "#button");
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
    target: "//input[@id='search']"
    shadow_root:
      - "#outer"
      - "#inner"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Type { target, shadow_root, .. } => {
                assert_eq!(target.as_ref().unwrap(), "//input[@id='search']");
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
            Action::Wait { url, target, shadow_root, duration, .. } => {
                assert!(url.is_none());
                assert_eq!(target.as_ref().unwrap(), "//div[@class='loading']");
                assert_eq!(shadow_root.as_ref().unwrap(), &crate::flow::SelectorPath::Single("#shadow-container".to_string()));
                assert_eq!(*duration, Some(500));
            }
            _ => panic!("Expected Wait action"),
        }
    }

    #[test]
    fn test_parse_select() {
        let yaml = r#"
test:
  - select: "Option A"
    target: "//select[@id='country']"
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Select { value, target, values, .. } => {
                assert_eq!(value, "Option A");
                assert_eq!(target.as_ref().unwrap(), "//select[@id='country']");
                assert!(values.is_empty());
            }
            _ => panic!("Expected Select action"),
        }
    }

    #[test]
    fn test_parse_select_with_values() {
        let yaml = r#"
test:
  - select: "a"
    target: "//select"
    values:
      - a
      - b
"#;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Select { value, target, values, .. } => {
                assert_eq!(value, "a");
                assert_eq!(target.as_ref().unwrap(), "//select");
                assert_eq!(values, &["a", "b"]);
            }
            _ => panic!("Expected Select action"),
        }
    }

    #[test]
    fn test_parse_check() {
        let yaml = r##"
test:
  - check: "#terms"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Check { target, .. } => {
                assert_eq!(target.as_ref().unwrap(), "#terms");
            }
            _ => panic!("Expected Check action"),
        }
    }

    #[test]
    fn test_parse_uncheck() {
        let yaml = r##"
test:
  - uncheck: "#newsletter"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Uncheck { target, .. } => {
                assert_eq!(target.as_ref().unwrap(), "#newsletter");
            }
            _ => panic!("Expected Uncheck action"),
        }
    }

    #[test]
    fn test_parse_dblclick() {
        let yaml = r##"
test:
  - dblclick: "//button[@id='edit']"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::DblClick { target, .. } => {
                assert_eq!(target.as_ref().unwrap(), "//button[@id='edit']");
            }
            _ => panic!("Expected DblClick action"),
        }
    }

    #[test]
    fn test_parse_right_click() {
        let yaml = r##"
test:
  - right_click: "#context-menu"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::RightClick { target, .. } => {
                assert_eq!(target.as_ref().unwrap(), "#context-menu");
            }
            _ => panic!("Expected RightClick action"),
        }
    }

    #[test]
    fn test_parse_clear() {
        let yaml = r##"
test:
  - clear: "//input[@id='search']"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Clear { target, .. } => {
                assert_eq!(target.as_ref().unwrap(), "//input[@id='search']");
            }
            _ => panic!("Expected Clear action"),
        }
    }

    #[test]
    fn test_parse_upload() {
        let yaml = r##"
test:
  - upload: "/path/to/file.pdf"
    target: "//input[@type='file']"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Upload { files, target, .. } => {
                assert_eq!(files, &["/path/to/file.pdf"]);
                assert_eq!(target.as_ref().unwrap(), "//input[@type='file']");
            }
            _ => panic!("Expected Upload action"),
        }
    }

    #[test]
    fn test_parse_upload_multiple() {
        let yaml = r##"
test:
  - upload:
      - "/path/a.pdf"
      - "/path/b.png"
    target: "//input[@type='file']"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Upload { files, target, .. } => {
                assert_eq!(files, &["/path/a.pdf", "/path/b.png"]);
                assert_eq!(target.as_ref().unwrap(), "//input[@type='file']");
            }
            _ => panic!("Expected Upload action"),
        }
    }

    #[test]
    fn test_parse_drag() {
        let yaml = r##"
test:
  - drag: "#source"
    to: "#target"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Drag { source, to, .. } => {
                assert_eq!(source, "#source");
                assert_eq!(to, "#target");
            }
            _ => panic!("Expected Drag action"),
        }
    }

    #[test]
    fn test_parse_scroll() {
        let yaml = r##"
test:
  - scroll: "//section[@id='main']"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Scroll { target, x, y, .. } => {
                assert_eq!(target, "//section[@id='main']");
                assert!(*x == None);
                assert!(*y == None);
            }
            _ => panic!("Expected Scroll action"),
        }
    }

    #[test]
    fn test_parse_scroll_with_xy() {
        let yaml = r##"
test:
  - scroll: "//section[@id='main']"
    x: 0
    y: 200
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Scroll { target, x, y, .. } => {
                assert_eq!(target, "//section[@id='main']");
                assert_eq!(*x, Some(0));
                assert_eq!(*y, Some(200));
            }
            _ => panic!("Expected Scroll action"),
        }
    }

    #[test]
    fn test_parse_read_with_attribute() {
        let yaml = r##"
test:
  - read: "//a[@id='link']"
    to: LINK_HREF
    attribute: href
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Read { target, to, html, attribute, value, .. } => {
                assert_eq!(target.as_ref().unwrap(), "//a[@id='link']");
                assert_eq!(to.as_ref().unwrap(), "LINK_HREF");
                assert!(!*html);
                assert_eq!(attribute.as_ref().unwrap(), "href");
                assert!(!*value);
            }
            _ => panic!("Expected Read action"),
        }
    }

    #[test]
    fn test_parse_read_with_value_flag() {
        let yaml = r##"
test:
  - read: "//input[@id='field']"
    to: FIELD_VALUE
    value: true
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Read { target, to, html, attribute, value, .. } => {
                assert_eq!(target.as_ref().unwrap(), "//input[@id='field']");
                assert_eq!(to.as_ref().unwrap(), "FIELD_VALUE");
                assert!(!*html);
                assert!(attribute.is_none());
                assert!(*value);
            }
            _ => panic!("Expected Read action"),
        }
    }

    #[test]
    fn test_parse_dialog_accept() {
        let yaml = r##"
test:
  - dialog: accept
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Dialog { mode, prompt_text, timeout } => {
                assert_eq!(mode, "accept");
                assert!(prompt_text.is_none());
                assert!(*timeout == None);
            }
            _ => panic!("Expected Dialog action"),
        }
    }

    #[test]
    fn test_parse_dialog_dismiss() {
        let yaml = r##"
test:
  - dialog: dismiss
    timeout: 3000
    prompt_text: confirmed
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Dialog { mode, prompt_text, timeout } => {
                assert_eq!(mode, "dismiss");
                assert_eq!(prompt_text.as_ref().unwrap(), "confirmed");
                assert_eq!(*timeout, Some(3000));
            }
            _ => panic!("Expected Dialog action"),
        }
    }

    #[test]
    fn test_parse_download() {
        let yaml = r##"
test:
  - download: "./out.pdf"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Download { path, timeout, to } => {
                assert_eq!(path, "./out.pdf");
                assert!(*timeout == None);
                assert!(*to == None);
            }
            _ => panic!("Expected Download action"),
        }
    }

    #[test]
    fn test_parse_download_with_options() {
        let yaml = r##"
test:
  - download: "./report.pdf"
    timeout: 15000
    to: SAVED_PATH
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Download { path, timeout, to } => {
                assert_eq!(path, "./report.pdf");
                assert_eq!(*timeout, Some(15000));
                assert_eq!(to.as_ref().unwrap(), "SAVED_PATH");
            }
            _ => panic!("Expected Download action"),
        }
    }

    #[test]
    fn test_parse_for_with_list() {
        let yaml = r##"
test:
  - for: ["a", "b", "c"]
    as: ITEM
    do:
      - log: $ITEM
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::For { items, as_, do_ } => {
                assert!(matches!(items, serde_yaml::Value::Sequence(_)));
                assert_eq!(as_, "ITEM");
                assert_eq!(do_.len(), 1);
            }
            _ => panic!("Expected For action"),
        }
    }

    #[test]
    fn test_parse_for_with_count() {
        let yaml = r##"
test:
  - for: 3
    as: I
    do:
      - log: $I
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::For { items, as_, do_ } => {
                assert!(matches!(items, serde_yaml::Value::Number(_)));
                assert_eq!(as_, "I");
            }
            _ => panic!("Expected For action"),
        }
    }

    #[test]
    fn test_parse_while() {
        let yaml = r##"
test:
  - while:
      condition:
        xpath: //button[@id='more']
    do:
      - click: //button[@id='more']
    max: 10
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::While { condition, do_, max } => {
                assert!(matches!(condition, crate::flow::Condition::XPath(_)));
                assert_eq!(do_.len(), 1);
                assert_eq!(*max, Some(10));
            }
            _ => panic!("Expected While action"),
        }
    }

    #[test]
    fn test_parse_retry() {
        let yaml = r##"
test:
  - retry: 3
    interval: 1000
    do:
      - click: #flaky
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Retry { attempts, interval, do_ } => {
                assert_eq!(*attempts, 3);
                assert_eq!(*interval, Some(1000));
                assert_eq!(do_.len(), 1);
            }
            _ => panic!("Expected Retry action"),
        }
    }

    #[test]
    fn test_parse_assert() {
        let yaml = r##"
test:
  - assert:
      condition:
        xpath: //*[@id='success']
    message: "Login did not succeed"
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Assert { condition, message } => {
                assert!(matches!(condition, crate::flow::Condition::XPath(_)));
                assert_eq!(message.as_ref().unwrap(), "Login did not succeed");
            }
            _ => panic!("Expected Assert action"),
        }
    }

    #[test]
    fn test_parse_return() {
        let yaml = r##"
test:
  - return:
      token: $TOKEN
      status: ok
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Return { values } => {
                assert!(values.contains_key("token"));
                assert!(values.contains_key("status"));
            }
            _ => panic!("Expected Return action"),
        }
    }

    #[test]
    fn test_parse_fail() {
        let yaml = r##"
test:
  - fail: Something went wrong
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Fail { message } => {
                assert_eq!(message, "Something went wrong");
            }
            _ => panic!("Expected Fail action"),
        }
    }

    #[test]
    fn test_parse_save() {
        let yaml = r##"
test:
  - save: ./state.json
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Save { path } => {
                assert_eq!(path, "./state.json");
            }
            _ => panic!("Expected Save action"),
        }
    }

    #[test]
    fn test_parse_load() {
        let yaml = r##"
test:
  - load: ./state.json
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::Load { path } => {
                assert_eq!(path, "./state.json");
            }
            _ => panic!("Expected Load action"),
        }
    }

    #[test]
    fn test_parse_if_with_visible() {
        let yaml = r##"
test:
  - if:
      visible: "#spinner"
    then:
      - wait: 1000
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::If { condition, then, .. } => {
                assert!(matches!(condition, crate::flow::Condition::Visible(_)));
                assert_eq!(then.len(), 1);
            }
            _ => panic!("Expected If action"),
        }
    }

    #[test]
    fn test_parse_if_with_equals() {
        let yaml = r##"
test:
  - if:
      equals:
        var: $STATUS
        value: ok
    then:
      - log: success
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::If { condition, then, .. } => {
                assert!(matches!(condition, crate::flow::Condition::Equals { .. }));
                assert_eq!(then.len(), 1);
            }
            _ => panic!("Expected If action"),
        }
    }

    #[test]
    fn test_parse_if_with_and() {
        let yaml = r##"
test:
  - if:
      and:
        - xpath: //button[@id='a']
        - visible: "#modal"
    then:
      - log: both
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::If { condition, then, .. } => {
                assert!(matches!(condition, crate::flow::Condition::And(_)));
                assert_eq!(then.len(), 1);
            }
            _ => panic!("Expected If action"),
        }
    }

    #[test]
    fn test_parse_if_with_or() {
        let yaml = r##"
test:
  - if:
      or:
        - xpath: //button[@id='a']
        - css: "#fallback"
    then:
      - click: //button
"##;
        let flow_file = parse_str(yaml).unwrap();
        let actions = flow_file.get("test").unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Action::If { condition, then, .. } => {
                assert!(matches!(condition, crate::flow::Condition::Or(_)));
                assert_eq!(then.len(), 1);
            }
            _ => panic!("Expected If action"),
        }
    }
}
