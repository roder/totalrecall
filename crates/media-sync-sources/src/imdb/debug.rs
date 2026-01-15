use anyhow::Result;
use chromiumoxide::Page;
use browser_debug::{ServiceDebugConfig, ElementInfo};
use tracing::debug;

pub struct ImdbDebugConfig;

impl ImdbDebugConfig {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ServiceDebugConfig for ImdbDebugConfig {
    fn get_key_selectors(&self) -> Vec<String> {
        vec![
            "[data-testid=\"hero-rating-bar__user-rating\"]".to_string(),
            "[data-testid=\"hero-rating-bar__user-rating\"] button".to_string(),
            ".ipc-rating-prompt".to_string(),
            "button[aria-label*=\"Rate\"]".to_string(),
            "[data-testid=\"hero-rating-bar__user-rating__score\"]".to_string(),
        ]
    }
    
    fn get_verification_selectors(&self) -> Vec<String> {
        vec![
            "[data-testid=\"hero-rating-bar__user-rating\"]".to_string(),
            "[data-testid=\"hero-rating-bar__user-rating__score\"]".to_string(),
        ]
    }
    
    async fn verify_action(&self, action: &str, page: &Page) -> Result<bool> {
        match action {
            "set_rating" => {
                // Verify that rating was set by checking if the rating score element exists
                // and has a value
                match page.find_element("[data-testid=\"hero-rating-bar__user-rating__score\"]").await {
                    Ok(element) => {
                        let text = element.inner_text().await.ok().flatten();
                        debug!("Rating score element text: {:?}", text);
                        Ok(text.is_some() && !text.as_ref().unwrap().is_empty())
                    }
                    Err(_) => {
                        // Rating might not be set yet, or element doesn't exist
                        Ok(false)
                    }
                }
            }
            "click_rating_button" => {
                // Verify that rating dialog appeared
                match page.find_element(".ipc-rating-prompt").await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            }
            _ => {
                // Unknown action, assume success
                Ok(true)
            }
        }
    }
    
    async fn inspect_service_elements(&self, page: &Page) -> Result<Vec<ElementInfo>> {
        let selectors = self.get_key_selectors();
        let mut results = Vec::new();
        
        for selector in selectors {
            match page.find_element(selector.clone()).await {
                Ok(element) => {
                    let classes = element.attribute("class").await.ok().flatten();
                    let aria_label = element.attribute("aria-label").await.ok().flatten();
                    let aria_disabled = element.attribute("aria-disabled").await.ok().flatten();
                    let disabled = element.attribute("disabled").await.is_ok();
                    let text = element.inner_text().await.ok().flatten();
                    let inner_html = element.inner_html().await.ok().flatten();
                    let bounding_box = element.bounding_box().await.ok().map(|bbox| {
                        use browser_debug::inspector::BoundingBox;
                        BoundingBox {
                            x: bbox.x,
                            y: bbox.y,
                            width: bbox.width,
                            height: bbox.height,
                        }
                    });
                    let visible = bounding_box.as_ref()
                        .map(|bbox| bbox.width > 0.0 && bbox.height > 0.0)
                        .unwrap_or(false);
                    
                    results.push(ElementInfo {
                        selector,
                        exists: true,
                        visible,
                        classes,
                        aria_label,
                        aria_disabled,
                        disabled,
                        text,
                        inner_html: inner_html.as_ref().map(|html| {
                            if html.len() > 500 {
                                format!("{}...", &html[..500])
                            } else {
                                html.clone()
                            }
                        }),
                        bounding_box,
                    });
                }
                Err(_) => {
                    results.push(ElementInfo {
                        selector,
                        exists: false,
                        visible: false,
                        classes: None,
                        aria_label: None,
                        aria_disabled: None,
                        disabled: false,
                        text: None,
                        inner_html: None,
                        bounding_box: None,
                    });
                }
            }
        }
        
        Ok(results)
    }
    
    fn get_expected_state_after_action(&self, action: &str) -> Option<serde_json::Value> {
        match action {
            "set_rating" => {
                Some(serde_json::json!({
                    "rating_dialog_closed": true,
                    "rating_set": true,
                }))
            }
            "click_rating_button" => {
                Some(serde_json::json!({
                    "rating_dialog_visible": true,
                }))
            }
            _ => None,
        }
    }
}

