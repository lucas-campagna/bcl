use crate::flow::Selector;
use crate::error::Error;

pub fn detect(raw: impl Into<String>) -> Selector {
    Selector::new(raw)
}

pub fn parse_selector(xpath: Option<String>, css: Option<String>) -> Result<Option<Selector>, Error> {
    match (xpath, css) {
        (Some(s), None) => Ok(Some(Selector::XPath(s))),
        (None, Some(s)) => Ok(Some(Selector::Css(s))),
        (Some(x), Some(c)) => Err(Error::SelectorParse(format!(
            "Both xpath and css provided: xpath={}, css={}", x, c
        ))),
        (None, None) => Ok(None),
    }
}

pub fn get_selector_string(selector: &Option<Selector>) -> Option<String> {
    selector.as_ref().map(|s| s.as_str().to_string())
}
