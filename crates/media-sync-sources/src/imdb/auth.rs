use anyhow::{anyhow, Result};
use chromiumoxide::Page;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

const IMDB_HOMEPAGE: &str = "https://www.imdb.com/";
const IMDB_SIGNIN_PAGE: &str = "https://www.imdb.com/registration/signin/?subPageType=sign_in";
const SIGNIN_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
const PAGE_LOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Check if user is already signed in to IMDB
/// Returns true if signed in, false otherwise
pub async fn is_signed_in(page: &Page) -> Result<bool> {
    page.goto(IMDB_HOMEPAGE).await?;
    sleep(Duration::from_secs(2)).await;

    // Check if sign-in link exists (user is NOT signed in)
    // If sign-in link doesn't exist, user IS signed in
    match page.find_element(".nav__userMenu a.imdb-header__signin-text".to_string()).await {
        Ok(_) => {
            // Sign-in link exists, user is not signed in
            Ok(false)
        }
        Err(_) => {
            // Sign-in link doesn't exist, verify user menu exists (user is signed in)
            match page.find_element(".nav__userMenu".to_string()).await {
                Ok(_) => {
                    info!("User is already signed in to IMDB");
                    Ok(true)
                }
                Err(_) => {
                    // User menu doesn't exist either, assume not signed in
                    Ok(false)
                }
            }
        }
    }
}

/// Authenticate to IMDB with username and password
pub async fn authenticate(page: &Page, username: &str, password: &str) -> Result<()> {
    // Check if already signed in
    if is_signed_in(page).await? {
        return Ok(());
    }

    info!("Not signed in, proceeding with login...");
    sleep(Duration::from_secs(2)).await;

    // Navigate to sign-in page
    page.goto(IMDB_SIGNIN_PAGE).await?;
    sleep(Duration::from_secs(3)).await;

    // Check current URL to see if we're on the sign-in options page
    let current_url = page.url().await?.unwrap_or_default();
    let current_url_str = current_url.as_str();

    // If on sign-in options page, click "Sign in with IMDb" button
    if current_url_str.contains("/registration/signin") && !current_url_str.contains("/ap/signin") {
        match click_signin_option(page).await {
            Ok(_) => {
                info!("Clicked 'Sign in with IMDb' button");
                sleep(Duration::from_secs(3)).await;
            }
            Err(e) => {
                warn!("Could not find sign-in option button: {}", e);
                // Continue anyway, hoping form will appear
            }
        }
    }

    // Wait for page to be ready
    wait_for_page_ready(page).await?;
    sleep(Duration::from_secs(2)).await;

    // Find email and password inputs
    let email_input = find_email_input(page).await?;
    let password_input = find_password_input(page).await?;

    // Fill in credentials
    email_input.type_str(username).await?;
    password_input.type_str(password).await?;

    // Find and click submit button
    let submit_button = find_submit_button(page).await?;
    submit_button.click().await?;

    info!("Submitted login form, waiting for authentication...");
    sleep(Duration::from_secs(3)).await;

    // Check current page state before navigating away
    let post_submit_url = page.url().await?.unwrap_or_default();
    let post_submit_url_str = post_submit_url.as_str();
    info!("Post-submit URL: {}", post_submit_url_str);

    // Check for error messages on the current page
    let error_diagnostics = check_for_errors(page).await;
    if let Some(error_info) = &error_diagnostics {
        warn!("Error detected on page: {}", error_info);
        
        // If we get a passkey error, try to switch to password authentication
        if error_info.to_lowercase().contains("passkey") {
            info!("Passkey error detected, attempting to switch to password authentication...");
            match switch_to_password_auth(page).await {
                Ok(_) => {
                    info!("Switched to password authentication, retrying login...");
                    sleep(Duration::from_secs(2)).await;
                    
                    // Wait for page to be ready after navigation
                    wait_for_page_ready(page).await?;
                    sleep(Duration::from_secs(2)).await;
                    
                    // Check if we're on the sign-in form page (not password assistance page)
                    let current_url = page.url().await?.unwrap_or_default();
                    let current_url_str = current_url.as_str();
                    
                    // If we're on password assistance or reset page, navigate back
                    if current_url_str.contains("password") && 
                       (current_url_str.contains("assistance") || current_url_str.contains("reset") || current_url_str.contains("forgot")) {
                        warn!("Navigated to password assistance page instead of sign-in form. Going back to sign-in page...");
                        page.goto(IMDB_SIGNIN_PAGE).await?;
                        sleep(Duration::from_secs(2)).await;
                        click_signin_option(page).await?;
                        sleep(Duration::from_secs(2)).await;
                        wait_for_page_ready(page).await?;
                        sleep(Duration::from_secs(2)).await;
                    }
                    
                    // Re-find and fill in credentials
                    let email_input = find_email_input(page).await?;
                    let password_input = find_password_input(page).await?;
                    
                    // Clear existing values by clicking and selecting all, then typing
                    // This is more reliable than trying to use JavaScript to set value
                    email_input.click().await?;
                    // Triple-click to select all (works cross-platform)
                    email_input.click().await?;
                    email_input.click().await?;
                    email_input.type_str(username).await?;
                    
                    password_input.click().await?;
                    password_input.click().await?;
                    password_input.click().await?;
                    password_input.type_str(password).await?;
                    
                    // Find and click submit button again
                    let submit_button = find_submit_button(page).await?;
                    submit_button.click().await?;
                    
                    info!("Resubmitted login form with password authentication...");
                    sleep(Duration::from_secs(3)).await;
                }
                Err(e) => {
                    warn!("Could not switch to password authentication: {}. Will continue with error diagnostics.", e);
                }
            }
        }
    }

    // Navigate to homepage and verify sign-in
    page.goto(IMDB_HOMEPAGE).await?;
    sleep(Duration::from_secs(2)).await;

    // Verify sign-in
    if !is_signed_in(page).await? {
        // Gather diagnostic information
        let current_url = page.url().await?.unwrap_or_default();
        let page_title = page.evaluate("document.title").await
            .and_then(|r| Ok(r.value().and_then(|v| v.as_str().map(|s| s.to_string()))))
            .unwrap_or(None)
            .unwrap_or_default();
        
        // Try to get page text to see what's displayed
        let page_text = page.evaluate("document.body.innerText").await
            .and_then(|r| Ok(r.value().and_then(|v| v.as_str().map(|s| s.to_string()))))
            .unwrap_or(None);
        
        let mut error_msg = format!(
            "Failed to sign in to IMDB.\n\n\
            Diagnostic Information:\n\
            - Post-submit URL: {}\n\
            - Current URL: {}\n\
            - Page title: {}\n",
            post_submit_url_str,
            current_url.as_str(),
            page_title
        );

        if let Some(error_info) = error_diagnostics {
            error_msg.push_str(&format!("- Error detected: {}\n", error_info));
        }

        if let Some(text) = page_text {
            // Look for common error indicators in page text
            let text_lower = text.to_lowercase();
            if text_lower.contains("captcha") || text_lower.contains("verify") {
                error_msg.push_str("\n⚠️  CAPTCHA detected on page!\n");
            }
            if text_lower.contains("incorrect") || text_lower.contains("wrong password") || text_lower.contains("invalid") {
                error_msg.push_str("\n⚠️  Credential error detected on page!\n");
            }
            if text_lower.contains("suspicious") || text_lower.contains("unusual activity") {
                error_msg.push_str("\n⚠️  Suspicious activity warning detected!\n");
            }
            
            // Include first 500 chars of page text for debugging
            let preview: String = text.chars().take(500).collect();
            error_msg.push_str(&format!("\nPage content preview (first 500 chars):\n{}\n", preview));
        }

        error_msg.push_str(
            "\nPossible causes:\n\
            - IMDB captcha check triggered\n\
            - Incorrect IMDB login credentials\n\
            - Suspicious activity detected by IMDB\n\
            \n\
            If your login is correct, the issue is likely due to an IMDB captcha check.\n\
            To resolve this:\n\
            1. Log in to IMDB on your browser (preferably Chrome) on the same computer.\n\
            2. If already logged in, log out and log back in.\n\
            3. Complete any captcha checks that appear.\n\
            4. After successfully logging in, run the script again.\n\
            \n\
            For more details, see: https://github.com/RileyXX/IMDB-Trakt-Syncer/issues/2"
        );

        return Err(anyhow!(error_msg));
    }

    info!("Successfully signed in to IMDB");
    Ok(())
}

/// Click the "Sign in with IMDb" option button
async fn click_signin_option(page: &Page) -> Result<()> {
    // Try primary selector first
    match page.find_element("a[data-testid=\"sign_in_option_IMDB\"]".to_string()).await {
        Ok(button) => {
            button.click().await?;
            return Ok(());
        }
        Err(_) => {}
    }

    // Fallback to generic sign-in option
    match page.find_element("a[data-testid^=\"sign_in_option\"]".to_string()).await {
        Ok(button) => {
            button.click().await?;
            return Ok(());
        }
        Err(_) => {}
    }

    // Last resort: try any link that leads to /ap/signin
    match page.find_element("a[href*=\"/ap/signin\"]".to_string()).await {
        Ok(link) => {
            link.click().await?;
            return Ok(());
        }
        Err(_) => {}
    }

    Err(anyhow!("Could not find any sign-in option button or link"))
}

/// Wait for page ready state
async fn wait_for_page_ready(page: &Page) -> Result<()> {
    let ready_script = "document.readyState === 'complete'";
    
    // Poll for ready state with timeout
    let start = std::time::Instant::now();
    while start.elapsed() < PAGE_LOAD_TIMEOUT {
        match page.evaluate(ready_script).await {
            Ok(result) => {
                if let Some(value) = result.value() {
                    if value.as_bool().unwrap_or(false) {
                        return Ok(());
                    }
                }
            }
            Err(_) => {}
        }
        sleep(Duration::from_millis(100)).await;
    }

    // Even if timeout, continue (page might still be usable)
    warn!("Page ready state check timed out, continuing anyway");
    Ok(())
}

/// Find email input field
async fn find_email_input(page: &Page) -> Result<chromiumoxide::Element> {
    // Try multiple selectors
    let selectors = [
        "input[type='email']",
        "input[name*='email' i]",
        "input[id*='email' i]",
        "input[name*='userName' i]",
    ];

    for selector in &selectors {
        match page.find_element(selector.to_string()).await {
            Ok(element) => return Ok(element),
            Err(_) => continue,
        }
    }

    Err(anyhow!("Could not find email input field on sign-in page"))
}

/// Find password input field
async fn find_password_input(page: &Page) -> Result<chromiumoxide::Element> {
    // Try multiple selectors
    let selectors = [
        "input[type='password']",
        "input[name*='password' i]",
        "input[id*='password' i]",
    ];

    for selector in &selectors {
        match page.find_element(selector.to_string()).await {
            Ok(element) => return Ok(element),
            Err(_) => continue,
        }
    }

    Err(anyhow!("Could not find password input field on sign-in page"))
}

/// Find submit button
async fn find_submit_button(page: &Page) -> Result<chromiumoxide::Element> {
    // Try input[type='submit'] first
    match page.find_element("input[type='submit']".to_string()).await {
        Ok(button) => return Ok(button),
        Err(_) => {}
    }

    // Try button[type='submit']
    match page.find_element("button[type='submit']".to_string()).await {
        Ok(button) => return Ok(button),
        Err(_) => {}
    }

    // Try button with signIn in id
    match page.find_element("button[id*='signIn']".to_string()).await {
        Ok(button) => return Ok(button),
        Err(_) => {}
    }

    // Try button with submit in class
    match page.find_element("button[class*='submit']".to_string()).await {
        Ok(button) => return Ok(button),
        Err(_) => {}
    }

    // Last resort: find any button
    match page.find_element("button".to_string()).await {
        Ok(button) => return Ok(button),
        Err(_) => {}
    }

    Err(anyhow!("Could not find submit button on sign-in page"))
}

/// Check for error messages on the current page
async fn check_for_errors(page: &Page) -> Option<String> {
    // Common error selectors
    let error_selectors = [
        ".a-alert-content",
        ".a-alert-error",
        "[data-testid*='error']",
        ".error",
        "#auth-error-message-box",
        ".a-box-inner",
        "[role='alert']",
    ];

    for selector in &error_selectors {
        match page.find_element(selector.to_string()).await {
            Ok(element) => {
                if let Ok(text) = element.inner_text().await {
                    if let Some(text_str) = text {
                        if !text_str.trim().is_empty() {
                            return Some(format!("Error element found (selector: {}): {}", selector, text_str));
                        }
                    }
                }
            }
            Err(_) => continue,
        }
    }

    // Check for captcha indicators
    let captcha_selectors = [
        "[data-testid*='captcha']",
        "iframe[src*='captcha']",
        ".captcha",
        "#captcha",
    ];

    for selector in &captcha_selectors {
        match page.find_element(selector.to_string()).await {
            Ok(_) => {
                return Some(format!("CAPTCHA detected (selector: {})", selector));
            }
            Err(_) => continue,
        }
    }

    None
}

/// Switch from passkey authentication to password authentication
/// When IMDB shows a passkey error, it usually provides a link to "Sign in with password"
async fn switch_to_password_auth(page: &Page) -> Result<()> {
    // First, try to find links/buttons with specific data-testid attributes
    let testid_selectors = [
        "a[data-testid*='password']",
        "button[data-testid*='password']",
        "a[data-testid*='signin-password']",
        "button[data-testid*='signin-password']",
    ];

    for selector in &testid_selectors {
        match page.find_element(selector.to_string()).await {
            Ok(element) => {
                if let Ok(text) = element.inner_text().await {
                    if let Some(text_str) = text {
                        let text_lower = text_str.to_lowercase();
                        // Skip "Password assistance" - that's for password reset, not sign-in
                        if !text_lower.contains("assistance") && !text_lower.contains("reset") && !text_lower.contains("forgot") {
                            info!("Found password authentication option (testid): {}", text_str);
                            element.click().await?;
                            sleep(Duration::from_secs(2)).await;
                            return Ok(());
                        }
                    }
                }
            }
            Err(_) => continue,
        }
    }

    // Look for all links and buttons and check their text content
    // Prioritize links that say "sign in with password" or similar
    match page.find_elements("a, button".to_string()).await {
        Ok(elements) => {
            // First pass: look for exact matches
            for element in &elements {
                if let Ok(text) = element.inner_text().await {
                    if let Some(text_str) = text {
                        let text_lower = text_str.to_lowercase().trim().to_string();
                        // Look for exact patterns that indicate password sign-in (not password reset)
                        if (text_lower.contains("sign in") && text_lower.contains("password")) ||
                           (text_lower.contains("use password") && !text_lower.contains("assistance")) ||
                           (text_lower.contains("try password") && !text_lower.contains("assistance")) {
                            info!("Found password authentication option: {}", text_str);
                            element.click().await?;
                            sleep(Duration::from_secs(2)).await;
                            return Ok(());
                        }
                    }
                }
            }
            
            // Second pass: look for any password link that's not assistance/reset
            for element in &elements {
                if let Ok(text) = element.inner_text().await {
                    if let Some(text_str) = text {
                        let text_lower = text_str.to_lowercase().trim().to_string();
                        // Exclude password reset/assistance links
                        if text_lower.contains("password") && 
                           !text_lower.contains("passkey") &&
                           !text_lower.contains("assistance") &&
                           !text_lower.contains("reset") &&
                           !text_lower.contains("forgot") &&
                           !text_lower.contains("change") {
                            info!("Found potential password authentication option: {}", text_str);
                            element.click().await?;
                            sleep(Duration::from_secs(2)).await;
                            return Ok(());
                        }
                    }
                }
            }
        }
        Err(_) => {}
    }

    // If we can't find a password link, try navigating back to the sign-in page
    // This will reset the form and might avoid the passkey prompt
    warn!("Could not find password authentication link, navigating back to sign-in page...");
    page.goto(IMDB_SIGNIN_PAGE).await?;
    sleep(Duration::from_secs(2)).await;
    
    // Try clicking "Sign in with IMDb" again
    match click_signin_option(page).await {
        Ok(_) => {
            info!("Re-navigated to sign-in form");
            sleep(Duration::from_secs(2)).await;
            Ok(())
        }
        Err(e) => Err(anyhow!("Could not navigate back to sign-in form: {}", e))
    }
}
