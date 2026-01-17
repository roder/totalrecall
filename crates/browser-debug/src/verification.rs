use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum VerificationResult {
    Success,
    Failed(String),
    Inconclusive(String),
}

/// Verify that an action actually succeeded by comparing states
pub fn verify_action_result(
    before_state: &Value,
    after_state: &Value,
    expected_changes: &[&str],
) -> VerificationResult {
    let mut all_changes_found = true;
    let mut missing_changes = Vec::new();
    
    for change_key in expected_changes {
        let before_val = before_state.get(change_key);
        let after_val = after_state.get(change_key);
        
        if before_val == after_val {
            all_changes_found = false;
            missing_changes.push(change_key.to_string());
        }
    }
    
    if all_changes_found {
        VerificationResult::Success
    } else {
        VerificationResult::Failed(format!(
            "Expected changes not found: {}",
            missing_changes.join(", ")
        ))
    }
}

/// Compare two states and return differences
pub fn compare_states(before: &Value, after: &Value) -> Value {
    let mut differences = serde_json::Map::new();
    
    if before != after {
        differences.insert("changed".to_string(), Value::Bool(true));
        differences.insert("before".to_string(), before.clone());
        differences.insert("after".to_string(), after.clone());
    } else {
        differences.insert("changed".to_string(), Value::Bool(false));
    }
    
    Value::Object(differences)
}

/// Verify that an element changed as expected
pub fn verify_element_changed(
    before_info: &crate::inspector::ElementInfo,
    after_info: &crate::inspector::ElementInfo,
    expected_property: &str,
) -> VerificationResult {
    match expected_property {
        "visible" => {
            if !before_info.visible && after_info.visible {
                VerificationResult::Success
            } else if before_info.visible == after_info.visible {
                VerificationResult::Failed(format!(
                    "Element visibility did not change: {}",
                    before_info.visible
                ))
            } else {
                VerificationResult::Inconclusive("Visibility changed in unexpected way".to_string())
            }
        }
        "text" => {
            if before_info.text != after_info.text {
                VerificationResult::Success
            } else {
                VerificationResult::Failed("Element text did not change".to_string())
            }
        }
        "class" => {
            if before_info.classes != after_info.classes {
                VerificationResult::Success
            } else {
                VerificationResult::Failed("Element class did not change".to_string())
            }
        }
        _ => VerificationResult::Inconclusive(format!(
            "Unknown property to verify: {}",
            expected_property
        )),
    }
}

