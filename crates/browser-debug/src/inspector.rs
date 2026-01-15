use anyhow::{Context, Result};
use chromiumoxide::Page;
use chromiumoxide::page::ScreenshotParams;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, warn};
use crate::config::DebugConfig;

pub struct PageInspector {
    page: Page,
    config: DebugConfig,
    screenshot_counter: u32,
    step_counter: u32,
    current_step_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ElementInfo {
    pub selector: String,
    pub exists: bool,
    pub visible: bool,
    pub classes: Option<String>,
    pub aria_label: Option<String>,
    pub aria_disabled: Option<String>,
    pub disabled: bool,
    pub text: Option<String>,
    pub inner_html: Option<String>,
    pub bounding_box: Option<BoundingBox>,
}

#[derive(Debug, Clone)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl PageInspector {
    pub fn new(page: Page, config: DebugConfig) -> Result<Self> {
        // Ensure output directory exists
        std::fs::create_dir_all(&config.output_dir)
            .with_context(|| format!("Failed to create debug output directory: {:?}", config.output_dir))?;
        
        Ok(Self {
            page,
            config,
            screenshot_counter: 0,
            step_counter: 0,
            current_step_dir: None,
        })
    }
    
    /// Capture a screenshot with a label
    pub async fn screenshot(&mut self, label: &str) -> Result<PathBuf> {
        if !self.config.capture_screenshots {
            return Ok(PathBuf::from("screenshot_disabled"));
        }
        
        self.screenshot_counter += 1;
        let filename = format!("{:03}_{}.png", self.screenshot_counter, sanitize_label(label));
        let path = if let Some(ref step_dir) = self.current_step_dir {
            step_dir.join(&filename)
        } else {
            self.config.output_dir.join(&filename)
        };
        
        let params = ScreenshotParams::builder()
            .format(chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat::Png)
            .build();
        
        match self.page.screenshot(params).await {
            Ok(screenshot_data) => {
                std::fs::write(&path, screenshot_data)
                    .with_context(|| format!("Failed to write screenshot to {:?}", path))?;
                info!("Screenshot saved: {:?}", path);
                Ok(path)
            }
            Err(e) => {
                warn!("Failed to capture screenshot: {}", e);
                Err(e.into())
            }
        }
    }
    
    /// Inspect elements by selector, returning detailed information
    pub async fn inspect_elements(&self, selectors: &[&str]) -> Result<Vec<ElementInfo>> {
        let mut results = Vec::new();
        
        for selector in selectors {
            match self.page.find_element(selector.to_string()).await {
                Ok(element) => {
                    let classes = element.attribute("class").await.ok().flatten();
                    let aria_label = element.attribute("aria-label").await.ok().flatten();
                    let aria_disabled = element.attribute("aria-disabled").await.ok().flatten();
                    let disabled = element.attribute("disabled").await.is_ok();
                    let text = element.inner_text().await.ok().flatten();
                    let inner_html_raw = element.inner_html().await.ok().flatten();
                    let inner_html = inner_html_raw.as_ref().map(|html| {
                        // Truncate HTML for readability
                        if html.len() > 500 {
                            format!("{}...", &html[..500])
                        } else {
                            html.clone()
                        }
                    });
                    let bounding_box = element.bounding_box().await.ok().map(|bbox| BoundingBox {
                        x: bbox.x,
                        y: bbox.y,
                        width: bbox.width,
                        height: bbox.height,
                    });
                    let visible = bounding_box.as_ref()
                        .map(|bbox| bbox.width > 0.0 && bbox.height > 0.0)
                        .unwrap_or(false);
                    
                    results.push(ElementInfo {
                        selector: selector.to_string(),
                        exists: true,
                        visible,
                        classes,
                        aria_label,
                        aria_disabled,
                        disabled,
                        text,
                        inner_html,
                        bounding_box,
                    });
                }
                Err(_) => {
                    results.push(ElementInfo {
                        selector: selector.to_string(),
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
    
    /// Get comprehensive page state as JSON
    pub async fn get_page_state(&self) -> Result<Value> {
        let url = self.page.url().await?.map(|u| u.to_string()).unwrap_or_default();
        // Note: chromiumoxide Page doesn't have a direct title() method
        // We'll get it via JavaScript instead
        let title = String::new();
        
        // Get page state via JavaScript
        let js = r#"
        (() => {
            return {
                url: window.location.href,
                title: document.title,
                readyState: document.readyState,
                viewport: {
                    width: window.innerWidth,
                    height: window.innerHeight,
                },
                elements: {
                    body: document.body ? {
                        children: document.body.children.length,
                        innerHTML: document.body.innerHTML.substring(0, 1000),
                    } : null,
                },
            };
        })()
        "#;
        
        let mut state = json!({
            "url": url,
            "title": title,
        });
        
        match self.page.evaluate(js).await {
            Ok(result) => {
                if let Some(value) = result.value() {
                    if let Some(obj) = value.as_object() {
                        for (key, val) in obj {
                            state[key] = val.clone();
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to execute JavaScript for page state: {}", e);
            }
        }
        
        Ok(state)
    }
    
    /// Save full page HTML
    pub async fn save_page_html(&self, label: &str) -> Result<PathBuf> {
        if !self.config.capture_html {
            return Ok(PathBuf::from("html_disabled"));
        }
        
        let filename = format!("{}.html", sanitize_label(label));
        let path = if let Some(ref step_dir) = self.current_step_dir {
            step_dir.join(&filename)
        } else {
            self.config.output_dir.join(&filename)
        };
        
        let html = self.page.content().await?;
        std::fs::write(&path, html)
            .with_context(|| format!("Failed to write HTML to {:?}", path))?;
        info!("Page HTML saved: {:?}", path);
        Ok(path)
    }
    
    /// Capture browser console messages
    pub async fn capture_console_logs(&self) -> Result<Vec<Value>> {
        if !self.config.capture_console {
            return Ok(Vec::new());
        }
        
        // Note: chromiumoxide doesn't have direct console log capture
        // We can use CDP Runtime.consoleAPICalled event, but for now
        // we'll return an empty vec and note this as a future enhancement
        // The actual implementation would require setting up event listeners
        Ok(Vec::new())
    }
    
    /// Capture network requests (placeholder - requires CDP Network domain)
    pub async fn capture_network_requests(&self) -> Result<Vec<Value>> {
        if !self.config.capture_network {
            return Ok(Vec::new());
        }
        
        // Note: This would require enabling Network domain and listening to events
        // For now, return empty vec
        Ok(Vec::new())
    }
    
    /// Wait for element to appear with timeout
    pub async fn wait_for_element(
        &self,
        selector: &str,
        timeout_seconds: u64,
    ) -> Result<bool> {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(timeout_seconds);
        
        while start.elapsed() < timeout {
            match self.page.find_element(selector.to_string()).await {
                Ok(_) => return Ok(true),
                Err(_) => {
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }
        
        Ok(false)
    }
    
    /// Verify element state matches expected properties
    pub async fn verify_element_state(
        &self,
        selector: &str,
        expected_properties: &HashMap<String, String>,
    ) -> Result<bool> {
        match self.page.find_element(selector.to_string()).await {
            Ok(element) => {
                for (key, expected_value) in expected_properties.iter() {
                    let actual_value = match key.as_str() {
                        "aria-disabled" => element.attribute("aria-disabled").await.ok().flatten(),
                        "disabled" => {
                            if element.attribute("disabled").await.is_ok() {
                                Some("true".to_string())
                            } else {
                                Some("false".to_string())
                            }
                        }
                        "class" => element.attribute("class").await.ok().flatten(),
                        "text" => element.inner_text().await.ok().flatten(),
                        _ => element.attribute(key).await.ok().flatten(),
                    };
                    
                    if actual_value.as_ref().map(|v| v.as_str()) != Some(expected_value.as_str()) {
                        debug!(
                            "Element property mismatch: {} expected '{}', got '{:?}'",
                            key, expected_value, actual_value
                        );
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }
    
    /// Set the current step directory for organizing debug output
    pub fn set_step_dir(&mut self, step_dir: PathBuf) {
        self.current_step_dir = Some(step_dir);
    }
    
    /// Clear the current step directory
    pub fn clear_step_dir(&mut self) {
        self.current_step_dir = None;
    }
    
    /// Get the current step counter
    pub fn step_counter(&self) -> u32 {
        self.step_counter
    }
    
    /// Increment step counter
    pub fn increment_step(&mut self) -> u32 {
        self.step_counter += 1;
        self.step_counter
    }
    
    /// Get reference to the underlying page
    pub fn page(&self) -> &Page {
        &self.page
    }
    
    /// Get reference to the debug config
    pub fn config(&self) -> &DebugConfig {
        &self.config
    }
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

