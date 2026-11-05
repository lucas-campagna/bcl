use crate::cdp::client::CdpClient;
use crate::cdp::options::{ClickOptions, FillOptions};
use crate::cdp::page::{
    count_nodes, dispatch_mouse, element_center, element_expr, evaluate_raw, evaluate_value,
    js_str, locator_visible,
};
use crate::error::Error;
use std::sync::Arc;

pub struct Locator {
    client: Arc<CdpClient>,
    selector: String,
    index: Option<usize>,
}

impl Locator {
    pub fn new(client: Arc<CdpClient>, selector: String, index: Option<usize>) -> Self {
        Locator {
            client,
            selector,
            index,
        }
    }

    pub fn first(&self) -> Locator {
        Locator {
            client: self.client.clone(),
            selector: self.selector.clone(),
            index: Some(self.index.unwrap_or(0)),
        }
    }

    pub async fn all(&self) -> Result<Vec<Locator>, Error> {
        let count = count_nodes(&self.client, &self.selector).await?;
        Ok((0..count)
            .map(|i| Locator {
                client: self.client.clone(),
                selector: self.selector.clone(),
                index: Some(i),
            })
            .collect())
    }

    pub async fn count(&self) -> Result<usize, Error> {
        count_nodes(&self.client, &self.selector).await
    }

    pub async fn fill(&self, value: &str, _opts: Option<FillOptions>) -> Result<(), Error> {
        let expr = format!(
            "(function(){{var el={el};if(!el)return false;el.focus();el.value={value};el.dispatchEvent(new Event('input',{{bubbles:true}}));el.dispatchEvent(new Event('change',{{bubbles:true}}));return true;}})()",
            el = element_expr(&self.selector, self.index),
            value = js_str(value)
        );
        evaluate_raw(&self.client, &expr).await?;
        Ok(())
    }

    pub async fn clear(&self) -> Result<(), Error> {
        let expr = format!(
            "(function(){{var el={el};if(!el)return false;el.focus();el.value='';el.dispatchEvent(new Event('input',{{bubbles:true}}));el.dispatchEvent(new Event('change',{{bubbles:true}}));return true;}})()",
            el = element_expr(&self.selector, self.index)
        );
        evaluate_raw(&self.client, &expr).await?;
        Ok(())
    }

    pub async fn inner_text(&self) -> Result<String, Error> {
        let expr = format!(
            "(function(){{var el={el};return el?(el.innerText||el.textContent||''):'';}})()",
            el = element_expr(&self.selector, self.index)
        );
        let value = evaluate_value(&self.client, &expr).await?;
        Ok(value.as_str().unwrap_or("").to_string())
    }

    pub async fn inner_html(&self) -> Result<String, Error> {
        let expr = format!(
            "(function(){{var el={el};return el?(el.innerHTML||''):'';}})()",
            el = element_expr(&self.selector, self.index)
        );
        let value = evaluate_value(&self.client, &expr).await?;
        Ok(value.as_str().unwrap_or("").to_string())
    }

    pub async fn input_value(&self) -> Result<String, Error> {
        let expr = format!(
            "(function(){{var el={el};return el?(el.value===undefined?'':String(el.value)):'';}})()",
            el = element_expr(&self.selector, self.index)
        );
        let value = evaluate_value(&self.client, &expr).await?;
        Ok(value.as_str().unwrap_or("").to_string())
    }

    pub async fn get_attribute(&self, name: &str) -> Result<Option<String>, Error> {
        let expr = format!(
            "(function(){{var el={el};if(!el)return null;var v=el.getAttribute({name});return v===null?null:String(v);}})()",
            el = element_expr(&self.selector, self.index),
            name = js_str(name)
        );
        let value = evaluate_value(&self.client, &expr).await?;
        Ok(value.as_str().map(|s| s.to_string()))
    }

    pub async fn scroll_into_view_if_needed(&self) -> Result<(), Error> {
        let expr = format!(
            "(function(){{var el={el};if(!el)return false;el.scrollIntoView({{block:'center',inline:'center'}});return true;}})()",
            el = element_expr(&self.selector, self.index)
        );
        evaluate_raw(&self.client, &expr).await?;
        Ok(())
    }

    pub async fn right_click(&self) -> Result<(), Error> {
        let (x, y) = element_center(&self.client, &self.selector, self.index).await?;
        dispatch_mouse(&self.client, "mouseMoved", x, y, "none", 0, 0).await?;
        dispatch_mouse(&self.client, "mousePressed", x, y, "right", 1, 2).await?;
        dispatch_mouse(&self.client, "mouseReleased", x, y, "right", 1, 2).await?;
        Ok(())
    }

    pub async fn click(&self, _opts: Option<ClickOptions>) -> Result<(), Error> {
        let (x, y) = element_center(&self.client, &self.selector, self.index).await?;
        dispatch_mouse(&self.client, "mouseMoved", x, y, "none", 0, 0).await?;
        dispatch_mouse(&self.client, "mousePressed", x, y, "left", 1, 1).await?;
        dispatch_mouse(&self.client, "mouseReleased", x, y, "left", 1, 1).await?;
        Ok(())
    }

    pub async fn is_visible(&self) -> Result<bool, Error> {
        locator_visible(&self.client, &self.selector, self.index).await
    }

    pub async fn is_checked(&self) -> Result<bool, Error> {
        let expr = format!(
            "(function(){{var el={el};return el?!!el.checked:false;}})()",
            el = element_expr(&self.selector, self.index)
        );
        let value = evaluate_value(&self.client, &expr).await?;
        Ok(value.as_bool().unwrap_or(false))
    }

    pub async fn is_enabled(&self) -> Result<bool, Error> {
        let expr = format!(
            "(function(){{var el={el};return el?!el.disabled:false;}})()",
            el = element_expr(&self.selector, self.index)
        );
        let value = evaluate_value(&self.client, &expr).await?;
        Ok(value.as_bool().unwrap_or(false))
    }
}
