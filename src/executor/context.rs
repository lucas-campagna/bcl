use crate::flow::Context;
use std::env;

fn is_broker_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    if let Some(first) = chars.next() {
        first.is_uppercase() || first == '_'
    } else {
        false
    }
}

impl Context {
    pub fn from_env() -> Self {
        let mut ctx = Context::new();
        if let Ok(vars) = env::var("BROKER_VARS") {
            for line in vars.split(',') {
                if let Some((k, v)) = line.split_once('=') {
                    let yaml_value = Context::try_parse_yaml(v.trim());
                    ctx.set_yaml(k.trim(), yaml_value);
                }
            }
        }
        for (k, v) in env::vars() {
            if is_broker_var_name(&k) && !k.starts_with("BROKER_") {
                let yaml_value = Context::try_parse_yaml(&v);
                ctx.set_yaml(k, yaml_value);
            }
        }
        ctx
    }
}
