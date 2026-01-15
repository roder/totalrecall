use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::env;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScreenshotFormat {
    #[serde(rename = "png")]
    Png,
    #[serde(rename = "jpeg")]
    Jpeg,
}

impl Default for ScreenshotFormat {
    fn default() -> Self {
        ScreenshotFormat::Png
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    
    #[serde(default = "default_true")]
    pub capture_screenshots: bool,
    
    #[serde(default = "default_true")]
    pub capture_html: bool,
    
    #[serde(default = "default_true")]
    pub capture_console: bool,
    
    #[serde(default = "default_false")]
    pub capture_network: bool,
    
    #[serde(default)]
    pub screenshot_format: ScreenshotFormat,
}

fn default_enabled() -> bool {
    env::var("BROWSER_DEBUG").map(|v| v == "1" || !v.is_empty()).unwrap_or(false)
}

fn default_output_dir() -> PathBuf {
    env::var("BROWSER_DEBUG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./browser_debug"))
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            output_dir: default_output_dir(),
            capture_screenshots: true,
            capture_html: true,
            capture_console: true,
            capture_network: false,
            screenshot_format: ScreenshotFormat::Png,
        }
    }
}

impl DebugConfig {
    /// Create a new DebugConfig from environment variables
    pub fn from_env() -> Self {
        Self::default()
    }
    
    /// Create a new DebugConfig with custom settings
    pub fn new(
        enabled: bool,
        output_dir: impl AsRef<Path>,
    ) -> Result<Self> {
        let output_dir = output_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&output_dir)
            .with_context(|| format!("Failed to create debug output directory: {:?}", output_dir))?;
        
        Ok(Self {
            enabled,
            output_dir,
            ..Default::default()
        })
    }
    
    /// Check if debugging is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    
    /// Get the output directory path
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }
}

