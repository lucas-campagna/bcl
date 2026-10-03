use crate::flow::Selector;
use crate::error::Error;

pub fn detect(raw: impl Into<String>) -> Selector {
    Selector::new(raw)
}

pub fn parse_selector(target: Option<String>) -> Result<Option<Selector>, Error> {
    match target {
        Some(s) => Ok(Some(Selector::new(s))),
        None => Ok(None),
    }
}

pub fn get_selector_string(selector: &Option<Selector>) -> Option<String> {
    selector.as_ref().map(|s| s.as_str().to_string())
}
