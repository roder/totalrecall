pub mod inspector;
pub mod config;
pub mod verification;
pub mod workflow;
pub mod service_config;

pub use inspector::{PageInspector, ElementInfo, BoundingBox};
pub use config::DebugConfig;
pub use verification::{VerificationResult, verify_action_result, compare_states};
pub use workflow::DebugWorkflow;
pub use service_config::ServiceDebugConfig;

