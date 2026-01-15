use anyhow::Result;
use chromiumoxide::Page;
use crate::inspector::ElementInfo;

/// Trait for service-specific debug configurations
/// Allows each service to define its own debugging strategies
#[async_trait::async_trait]
pub trait ServiceDebugConfig: Send + Sync {
    /// Get key selectors that should be inspected at each step
    fn get_key_selectors(&self) -> Vec<String>;
    
    /// Get selectors that should be verified after actions
    fn get_verification_selectors(&self) -> Vec<String>;
    
    /// Verify that an action actually succeeded
    /// Returns true if the action succeeded, false otherwise
    async fn verify_action(&self, action: &str, page: &Page) -> Result<bool>;
    
    /// Get service-specific element information for debugging
    async fn inspect_service_elements(&self, page: &Page) -> Result<Vec<ElementInfo>>;
    
    /// Get expected state after an action
    fn get_expected_state_after_action(&self, action: &str) -> Option<serde_json::Value>;
}
