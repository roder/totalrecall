# browser-debug

A generic debugging framework for chromiumoxide-based browser automation. This crate provides tools to inspect, debug, and troubleshoot browser interactions by capturing screenshots, HTML, console logs, network requests, and page state.

## Features

- **Screenshot Capture**: Automatically capture screenshots at key interaction points
- **HTML Dumps**: Save full page HTML for offline inspection
- **Page State Inspection**: Capture comprehensive page state (URL, title, viewport, DOM structure)
- **Element Inspection**: Detailed information about DOM elements (visibility, attributes, bounding boxes)
- **Console Logs**: Capture browser console messages (when available)
- **Network Requests**: Monitor network activity (when enabled)
- **Structured Workflows**: Organize debugging output by operation and step
- **Environment-Based Configuration**: Enable/disable via environment variables

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
browser-debug = { path = "../browser-debug" }
chromiumoxide = "0.5"
```

## Quick Start

### Basic Usage

```rust
use browser_debug::{PageInspector, DebugConfig};
use chromiumoxide::{Browser, BrowserConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Launch browser
    let (browser, mut handler) = Browser::launch(
        BrowserConfig::builder().build()?
    ).await?;
    
    // Create a page
    let page = browser.new_page("https://example.com").await?;
    
    // Create debug config (enabled via environment variable)
    let debug_config = DebugConfig::from_env();
    
    // Initialize inspector
    let mut inspector = PageInspector::new(page.clone(), debug_config)?;
    
    // Capture screenshot
    inspector.screenshot("initial_page").await?;
    
    // Inspect elements
    let elements = inspector.inspect_elements(&[
        "button.submit",
        "input.email",
    ]).await?;
    
    for element in elements {
        println!("Element: {}", element.selector);
        println!("  Exists: {}", element.exists);
        println!("  Visible: {}", element.visible);
        if let Some(text) = element.text {
            println!("  Text: {}", text);
        }
    }
    
    // Get page state
    let state = inspector.get_page_state().await?;
    println!("Page state: {}", serde_json::to_string_pretty(&state)?);
    
    Ok(())
}
```

## Configuration

### Environment Variables

The easiest way to enable debugging is via environment variables:

```bash
# Enable debugging (any non-empty value enables it)
export BROWSER_DEBUG=1

# Optional: Set custom output directory (default: ./browser_debug)
export BROWSER_DEBUG_DIR=/path/to/debug/output
```

**Fish Shell:**
```fish
set -x BROWSER_DEBUG 1
set -x BROWSER_DEBUG_DIR /path/to/debug/output
```

### Programmatic Configuration

```rust
use browser_debug::DebugConfig;
use std::path::PathBuf;

// Create config from environment
let config = DebugConfig::from_env();

// Or create custom config
let config = DebugConfig::new(
    true,  // enabled
    PathBuf::from("./my_debug_output"),  // output directory
)?;

// Check if enabled
if config.is_enabled() {
    println!("Debugging enabled, output: {:?}", config.output_dir());
}
```

### Configuration Options

The `DebugConfig` struct supports the following options:

- `enabled`: Whether debugging is active
- `output_dir`: Directory where debug artifacts are saved
- `capture_screenshots`: Capture screenshots (default: `true`)
- `capture_html`: Save page HTML (default: `true`)
- `capture_console`: Capture console logs (default: `true`)
- `capture_network`: Capture network requests (default: `false`)
- `screenshot_format`: PNG or JPEG (default: PNG)

## Integration with TotalRecall IMDB Client

The browser-debug crate is integrated into TotalRecall's IMDB client for debugging browser automation issues. Here's how it works:

### Enabling Debug Mode

Enable debugging when running TotalRecall sync operations:

```bash
# Enable browser debugging
export BROWSER_DEBUG=1

# Run sync with verbose logging
cargo run -- sync -v --ratings
```

### How It Works

1. **Initialization**: When `ImdbClient::set_ratings()` is called, it checks if debugging is enabled
2. **Page Inspector Creation**: If enabled, a `PageInspector` is created with the same `Page` object used by the client
3. **Automatic Capture**: The inspector captures screenshots and HTML at key interaction points
4. **Output**: All debug artifacts are saved to the configured output directory

### Example: Debugging Rating Operations

When debugging IMDB rating operations, the inspector captures:

```rust
// In media-sync-sources/src/imdb/actions.rs

// After navigating to a movie page
if let Some(ref mut insp) = inspector {
    let _ = insp.screenshot("navigate_to_page").await;
    let _ = insp.save_page_html("navigate_to_page").await;
}

// Before clicking rating button
if let Some(ref mut insp) = inspector {
    let _ = insp.screenshot("before_click_rating_button").await;
}

// After attempting to set rating
if let Some(ref mut insp) = inspector {
    let _ = insp.screenshot("after_set_rating").await;
}
```

### Debug Output Structure

When debugging IMDB operations, the output directory will contain:

```
browser_debug/
├── 001_navigate_to_page.png
├── 001_navigate_to_page.html
├── 002_before_click_rating_button.png
├── 003_after_set_rating.png
└── ...
```

### Inspecting IMDB Pages

To debug specific IMDB page interactions:

```rust
use browser_debug::PageInspector;

// After navigating to an IMDB page
let mut inspector = PageInspector::new(page.clone(), debug_config)?;

// Capture initial state
inspector.screenshot("imdb_movie_page").await?;
inspector.save_page_html("imdb_movie_page").await?;

// Inspect rating button
let rating_elements = inspector.inspect_elements(&[
    "button[data-testid='tm-box-rating-button']",
    ".ipc-rating-button",
    "[data-testid='rating-button']",
]).await?;

for element in rating_elements {
    println!("Rating button found: {}", element.exists);
    println!("  Visible: {}", element.visible);
    println!("  Disabled: {}", element.disabled);
    if let Some(classes) = element.classes {
        println!("  Classes: {}", classes);
    }
}

// Get page state for debugging
let state = inspector.get_page_state().await?;
println!("Viewport: {}x{}", 
    state["viewport"]["width"], 
    state["viewport"]["height"]
);
```

### Debugging Network Requests

To debug network requests from IMDB:

```rust
use browser_debug::{PageInspector, DebugConfig};

let mut config = DebugConfig::from_env();
config.capture_network = true;  // Enable network capture

let mut inspector = PageInspector::new(page.clone(), config)?;

// Navigate to page
page.goto("https://www.imdb.com/title/tt1234567/").await?;

// Capture network requests
let requests = inspector.capture_network_requests().await?;
for request in requests {
    println!("Request: {:?}", request);
}
```

## Advanced Usage

### Structured Workflows

For complex operations, use `DebugWorkflow` to organize debugging output:

```rust
use browser_debug::{PageInspector, DebugWorkflow};

let mut inspector = PageInspector::new(page.clone(), debug_config)?;
let mut workflow = DebugWorkflow::new(&mut inspector, "set_rating");

// Start a step
workflow.start_step("navigate_to_page")?;
page.goto("https://www.imdb.com/title/tt1234567/").await?;
workflow.capture_step_state().await?;
workflow.end_step(true, None)?;

// Another step
workflow.start_step("click_rating_button")?;
// ... perform action ...
workflow.capture_step_state().await?;
workflow.end_step(true, None)?;

// Complete workflow
workflow.complete().await?;
```

This creates organized output:

```
browser_debug/
└── set_rating/
    ├── 001_navigate_to_page/
    │   ├── navigate_to_page.png
    │   ├── navigate_to_page.html
    │   └── page_state.json
    └── 002_click_rating_button/
        ├── click_rating_button.png
        └── page_state.json
```

### Element Verification

Verify element state before interactions:

```rust
use browser_debug::PageInspector;

let mut inspector = PageInspector::new(page.clone(), debug_config)?;

// Wait for element to appear
match inspector.wait_for_element("button.rating-button", Duration::from_secs(5)).await {
    Ok(element) => {
        println!("Element found!");
        
        // Verify it's enabled
        let info = inspector.inspect_elements(&["button.rating-button"]).await?;
        if let Some(element_info) = info.first() {
            if element_info.disabled {
                println!("Warning: Button is disabled!");
            }
        }
    }
    Err(e) => {
        println!("Element not found: {}", e);
    }
}
```

### Page State Comparison

Compare page states before and after actions:

```rust
use browser_debug::{PageInspector, compare_states};

let mut inspector = PageInspector::new(page.clone(), debug_config)?;

// Capture initial state
let before_state = inspector.get_page_state().await?;

// Perform action
page.find_element("button.submit").await?.click().await?;
sleep(Duration::from_secs(2)).await;

// Capture after state
let after_state = inspector.get_page_state().await?;

// Compare
let comparison = compare_states(&before_state, &after_state);
println!("State changed: {}", comparison.changed);
if comparison.changed {
    println!("Changes: {:?}", comparison.changes);
}
```

## Output Structure

### Flat Structure (Simple Usage)

When using `PageInspector` directly:

```
browser_debug/
├── 001_navigate_to_page.png
├── 001_navigate_to_page.html
├── 002_before_click.png
├── 003_after_click.png
└── page_state.json
```

### Hierarchical Structure (Workflow Usage)

When using `DebugWorkflow`:

```
browser_debug/
└── operation_name/
    ├── 001_step_name/
    │   ├── step_name.png
    │   ├── step_name.html
    │   └── page_state.json
    └── 002_another_step/
        └── ...
```

## Troubleshooting

### Screenshots Not Being Captured

1. Check that `BROWSER_DEBUG=1` is set
2. Verify `capture_screenshots` is `true` in config
3. Check that the output directory is writable
4. Look for error messages in logs

### HTML Not Being Saved

1. Ensure `capture_html` is enabled
2. Check file permissions on output directory
3. Verify the page has loaded (check `readyState` in page state)

### Element Inspection Returns Empty Results

1. Verify selectors are correct (use browser DevTools to test)
2. Check that elements exist in the DOM (not dynamically loaded)
3. Wait for page to fully load before inspecting
4. Use `wait_for_element()` for dynamic content

### Viewport Size Issues

The browser-debug screenshots show the actual viewport size used by the browser. By default, chromiumoxide uses 800x600. If you need a different viewport:

1. Set viewport size when creating pages (if chromiumoxide supports it)
2. Or configure the browser with explicit window size arguments

The screenshots accurately represent what the browser sees at that viewport size.

## API Reference

### PageInspector

Main debugging interface for inspecting pages.

- `new(page: Page, config: DebugConfig) -> Result<Self>`: Create a new inspector
- `screenshot(label: &str) -> Result<PathBuf>`: Capture screenshot
- `save_page_html(label: &str) -> Result<PathBuf>`: Save page HTML
- `inspect_elements(selectors: &[&str]) -> Result<Vec<ElementInfo>>`: Inspect DOM elements
- `get_page_state() -> Result<Value>`: Get comprehensive page state
- `wait_for_element(selector: &str, timeout: Duration) -> Result<Element>`: Wait for element

### DebugWorkflow

Structured workflow debugging.

- `new(inspector: &mut PageInspector, operation_name: &str) -> Self`: Create workflow
- `start_step(step_name: &str) -> Result<()>`: Begin a step
- `capture_step_state() -> Result<()>`: Capture all state for current step
- `end_step(success: bool, error: Option<String>) -> Result<()>`: End current step
- `complete() -> Result<()>`: Complete workflow

### DebugConfig

Configuration for debugging behavior.

- `from_env() -> Self`: Create from environment variables
- `new(enabled: bool, output_dir: impl AsRef<Path>) -> Result<Self>`: Create custom config
- `is_enabled() -> bool`: Check if debugging is enabled
- `output_dir() -> &Path`: Get output directory

## Examples

### Example 1: Basic Page Inspection

```rust
let mut inspector = PageInspector::new(page.clone(), DebugConfig::from_env())?;

// Navigate
page.goto("https://example.com").await?;

// Capture
inspector.screenshot("homepage").await?;
inspector.save_page_html("homepage").await?;

// Inspect
let buttons = inspector.inspect_elements(&["button"]).await?;
println!("Found {} buttons", buttons.len());
```

### Example 2: IMDB Rating Debugging

```rust
// In your IMDB client code
async fn set_ratings(&self, ratings: &[Rating]) -> Result<()> {
    let page = browser.new_page("about:blank").await?;
    
    let mut inspector_opt = if self.debug_config.is_enabled() {
        Some(PageInspector::new(page.clone(), self.debug_config.clone())?)
    } else {
        None
    };
    
    for rating in ratings {
        let url = format!("https://www.imdb.com/title/{}/", rating.imdb_id);
        page.goto(&url).await?;
        
        if let Some(ref mut insp) = inspector_opt {
            insp.screenshot("navigate_to_page").await?;
            insp.save_page_html("navigate_to_page").await?;
        }
        
        // ... perform rating action ...
        
        if let Some(ref mut insp) = inspector_opt {
            insp.screenshot("after_rating").await?;
        }
    }
    
    Ok(())
}
```

### Example 3: Workflow-Based Debugging

```rust
let mut inspector = PageInspector::new(page.clone(), debug_config)?;
let mut workflow = DebugWorkflow::new(&mut inspector, "imdb_rating_sync");

workflow.start_step("authenticate")?;
// ... authentication code ...
workflow.capture_step_state().await?;
workflow.end_step(true, None)?;

workflow.start_step("set_ratings")?;
// ... rating code ...
workflow.capture_step_state().await?;
workflow.end_step(true, None)?;

workflow.complete().await?;
```

## Contributing

When adding new debugging features:

1. Keep the API generic (not service-specific)
2. Add configuration options for new features
3. Document environment variable support
4. Include examples in this README

## License

See the main project LICENSE file.


