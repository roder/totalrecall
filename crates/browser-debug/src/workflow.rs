use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, info};
use crate::inspector::PageInspector;
use crate::verification::VerificationResult;

pub struct DebugWorkflow<'a> {
    inspector: &'a mut PageInspector,
    operation_name: String,
    current_step: Option<Step>,
    steps: Vec<StepInfo>,
}

struct Step {
    name: String,
    step_number: u32,
    step_dir: PathBuf,
}

struct StepInfo {
    name: String,
    step_number: u32,
    success: bool,
    error: Option<String>,
}

impl<'a> DebugWorkflow<'a> {
    pub fn new(inspector: &'a mut PageInspector, operation_name: &str) -> Self {
        Self {
            inspector,
            operation_name: operation_name.to_string(),
            current_step: None,
            steps: Vec::new(),
        }
    }
    
    /// Begin a debugging step
    pub fn start_step(&mut self, step_name: &str) -> Result<()> {
        let step_number = self.inspector.increment_step();
        let step_name_str = step_name.to_string();
        let step_dir = self.inspector.config().output_dir()
            .join(&self.operation_name)
            .join(format!("{:03}_{}", step_number, sanitize_label(&step_name_str)));
        
        std::fs::create_dir_all(&step_dir)
            .with_context(|| format!("Failed to create step directory: {:?}", step_dir))?;
        
        self.inspector.set_step_dir(step_dir.clone());
        
        self.current_step = Some(Step {
            name: step_name_str.clone(),
            step_number,
            step_dir,
        });
        
        info!("Starting debug step {}: {}", step_number, step_name);
        Ok(())
    }
    
    /// Capture all configured state for the current step
    pub async fn capture_step_state(&mut self) -> Result<()> {
        if let Some(ref step) = self.current_step {
            let step_name = step.name.clone();
            let step_dir = step.step_dir.clone();
            
            // Capture screenshot
            if self.inspector.config().capture_screenshots {
                self.inspector.screenshot(&step_name).await?;
            }
            
            // Capture page state
            let page_state = self.inspector.get_page_state().await?;
            let state_path = step_dir.join("page_state.json");
            std::fs::write(&state_path, serde_json::to_string_pretty(&page_state)?)?;
            
            // Capture HTML if enabled
            if self.inspector.config().capture_html {
                self.inspector.save_page_html(&step_name).await?;
            }
            
            // Capture console logs if enabled
            if self.inspector.config().capture_console {
                let console_logs = self.inspector.capture_console_logs().await?;
                let logs_path = step_dir.join("console_logs.json");
                std::fs::write(&logs_path, serde_json::to_string_pretty(&console_logs)?)?;
            }
            
            debug!("Captured state for step {}: {:?}", step.step_number, step_dir);
        }
        Ok(())
    }
    
    /// Finalize the current step
    pub fn end_step(&mut self, success: bool, error: Option<String>) {
        if let Some(step) = self.current_step.take() {
            let step_name = step.name.clone();
            let step_number = step.step_number;
            
            self.steps.push(StepInfo {
                name: step.name,
                step_number: step.step_number,
                success,
                error: error.clone(),
            });
            
            if success {
                info!("Step {} completed successfully: {}", step_number, step_name);
            } else {
                let error_msg = error.as_deref().unwrap_or("Unknown error");
                info!("Step {} failed: {} - {}", step_number, step_name, error_msg);
            }
        }
        self.inspector.clear_step_dir();
    }
    
    /// Verify that the step succeeded
    pub async fn verify_step(
        &mut self,
        verification_selectors: &[&str],
        expected_properties: Option<&HashMap<String, String>>,
    ) -> VerificationResult {
        if let Some(ref step) = self.current_step {
            // Inspect key elements
            match self.inspector.inspect_elements(verification_selectors).await {
                Ok(elements) => {
                    // If expected properties provided, verify them
                    if let Some(expected) = expected_properties {
                        for (idx, selector) in verification_selectors.iter().enumerate() {
                            if let Some(element) = elements.get(idx) {
                                if element.exists {
                                    match self.inspector.verify_element_state(selector, expected).await {
                                        Ok(true) => {
                                            debug!("Element {} verified successfully", selector);
                                        }
                                        Ok(false) => {
                                            return VerificationResult::Failed(format!(
                                                "Element {} did not match expected properties",
                                                selector
                                            ));
                                        }
                                        Err(e) => {
                                            return VerificationResult::Inconclusive(format!(
                                                "Failed to verify element {}: {}",
                                                selector, e
                                            ));
                                        }
                                    }
                                } else {
                                    return VerificationResult::Failed(format!(
                                        "Element {} not found",
                                        selector
                                    ));
                                }
                            }
                        }
                    }
                    
                    VerificationResult::Success
                }
                Err(e) => {
                    VerificationResult::Inconclusive(format!(
                        "Failed to inspect elements: {}",
                        e
                    ))
                }
            }
        } else {
            VerificationResult::Inconclusive("No active step to verify".to_string())
        }
    }
    
    /// Get summary of all steps
    pub fn get_summary(&self) -> Value {
        let steps: Vec<Value> = self.steps.iter().map(|step| {
            json!({
                "step_number": step.step_number,
                "name": step.name,
                "success": step.success,
                "error": step.error,
            })
        }).collect();
        
        json!({
            "operation": self.operation_name,
            "total_steps": self.steps.len(),
            "successful_steps": self.steps.iter().filter(|s| s.success).count(),
            "failed_steps": self.steps.iter().filter(|s| !s.success).count(),
            "steps": steps,
        })
    }
}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

