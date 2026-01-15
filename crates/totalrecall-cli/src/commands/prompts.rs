use crate::output::Output;
use color_eyre::Result;
use dialoguer::{Confirm, Input, Password};

/// Prompt for a string value with optional default
pub fn prompt_string(prompt: &str, default: Option<&str>) -> Result<String> {
    let mut input_builder = Input::<String>::new()
        .with_prompt(prompt)
        .allow_empty(true);
    
    if let Some(default_value) = default {
        input_builder = input_builder.default(default_value.to_string());
    }
    
    input_builder.interact().map_err(|e| color_eyre::eyre::eyre!("Failed to read input: {}", e))
}

/// Prompt for a password (masked input)
pub fn prompt_password(prompt: &str) -> Result<String> {
    Password::new()
        .with_prompt(prompt)
        .interact()
        .map_err(|e| color_eyre::eyre::eyre!("Failed to read password: {}", e))
}

/// Prompt for yes/no with optional default
pub fn prompt_yes_no(prompt: &str, default: Option<bool>) -> Result<bool> {
    prompt_yes_no_with_output(prompt, default, None)
}

/// Prompt for yes/no with optional default and output handler
pub fn prompt_yes_no_with_output(prompt: &str, default: Option<bool>, output: Option<&Output>) -> Result<bool> {
    let mut confirm_builder = Confirm::new().with_prompt(prompt);
    
    if let Some(default_value) = default {
        confirm_builder = confirm_builder.default(default_value);
    }
    
    confirm_builder.interact().map_err(|e| {
        if let Some(out) = output {
            out.error(&format!("Failed to read confirmation: {}", e));
        }
        color_eyre::eyre::eyre!("Failed to read confirmation: {}", e)
    })
}

/// Prompt for a number with optional default
pub fn prompt_number(prompt: &str, default: Option<u32>) -> Result<u32> {
    prompt_number_with_output(prompt, default, None)
}

/// Prompt for a number with optional default and output handler
pub fn prompt_number_with_output(prompt: &str, default: Option<u32>, output: Option<&Output>) -> Result<u32> {
    loop {
        let mut input_builder = Input::<String>::new().with_prompt(prompt);
        
        if let Some(default_value) = default {
            input_builder = input_builder.default(default_value.to_string());
        }
        
        let input_str = input_builder.interact().map_err(|e| {
            if let Some(out) = output {
                out.error(&format!("Failed to read input: {}", e));
            }
            color_eyre::eyre::eyre!("Failed to read input: {}", e)
        })?;
        
        let trimmed = input_str.trim();
        
        if trimmed.is_empty() {
            if let Some(default_value) = default {
                return Ok(default_value);
            } else {
                if let Some(out) = output {
                    out.error("Invalid input. Please enter a valid number.");
                } else {
                    eprintln!("Invalid input. Please enter a valid number.");
                }
                continue;
            }
        }
        
        match trimmed.parse::<u32>() {
            Ok(num) => return Ok(num),
            Err(_) => {
                if let Some(out) = output {
                    out.error("Invalid input. Please enter a valid number.");
                } else {
                    eprintln!("Invalid input. Please enter a valid number.");
                }
                continue;
            }
        }
    }
}


