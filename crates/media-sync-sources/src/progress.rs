use std::collections::HashMap;
use tracing::{info, warn};

/// Progress tracker for operations that process multiple items
/// Provides periodic progress updates and final summaries to reduce log noise
pub struct ProgressTracker {
    total: usize,
    added: usize,
    already_present: usize,
    failed: usize,
    skipped: usize,
    start_time: std::time::Instant,
    progress_interval: usize, // Log every N items
    last_progress_log: usize,
    error_counts: HashMap<String, usize>, // Track errors by category
}

impl ProgressTracker {
    /// Create a new progress tracker
    /// 
    /// # Arguments
    /// * `total` - Total number of items to process
    /// * `progress_interval` - Log progress every N items (e.g., 50 for most operations, 25 for slower operations)
    pub fn new(total: usize, progress_interval: usize) -> Self {
        // Only log "Starting operation" if we expect it to take time
        // Skip for very small batches that are likely instant
        if total > 10 || progress_interval < total {
            info!("Starting operation: {} items to process", total);
        }
        Self {
            total,
            added: 0,
            already_present: 0,
            failed: 0,
            skipped: 0,
            start_time: std::time::Instant::now(),
            progress_interval,
            last_progress_log: 0,
            error_counts: HashMap::new(),
        }
    }

    /// Record that an item was successfully added
    pub fn record_added(&mut self) {
        self.added += 1;
    }

    /// Record that an item was already present (no action needed)
    pub fn record_already_present(&mut self) {
        self.already_present += 1;
    }

    /// Record that an item failed to process
    pub fn record_failed(&mut self) {
        self.failed += 1;
    }

    /// Record that an item failed to process with a specific error category
    /// This allows grouping errors by type in the summary
    pub fn record_failed_with_error(&mut self, error_category: &str) {
        self.failed += 1;
        *self.error_counts.entry(error_category.to_string()).or_insert(0) += 1;
    }

    /// Record that an item was skipped
    pub fn record_skipped(&mut self) {
        self.skipped += 1;
    }

    /// Log progress if interval has been reached
    /// Should be called after processing each item
    /// 
    /// # Arguments
    /// * `current` - Current item index (1-based, e.g., idx + 1 from enumerate)
    pub fn log_progress(&mut self, current: usize) {
        if current - self.last_progress_log >= self.progress_interval || current == self.total {
            let elapsed = self.start_time.elapsed();
            let rate = if elapsed.as_secs_f64() > 0.0 {
                current as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            };
            
            // Skip progress logs for operations that are too fast (< 0.5 seconds elapsed)
            // or have unrealistic rates (> 100k items/sec) - these are likely cache-only operations
            if elapsed.as_secs_f64() < 0.5 && current < self.total {
                return;
            }
            if rate > 100000.0 {
                return;
            }
            
            info!(
                "Progress: {}/{} ({:.1} items/sec) | Added: {} | Present: {} | Failed: {} | Skipped: {}",
                current, self.total, rate,
                self.added, self.already_present, self.failed, self.skipped
            );
            self.last_progress_log = current;
        }
    }

    /// Log final summary of the operation
    /// Should be called at the end of the operation
    /// 
    /// # Arguments
    /// * `operation_name` - Name of the operation (e.g., "IMDB watchlist add")
    pub fn log_summary(&self, operation_name: &str) {
        let elapsed = self.start_time.elapsed();
        // Only log summary if operation took meaningful time (> 0.1s)
        // or if there were failures/skipped items
        if elapsed.as_secs_f64() > 0.1 || self.failed > 0 || self.skipped > 0 {
            // Log as WARN if there are failures (actionable at summary level)
            // Otherwise log as INFO
            if self.failed > 0 {
                warn!(
                    "{} completed: {} total in {:.1}s | Added: {} | Already present: {} | Failed: {} | Skipped: {}",
                    operation_name, self.total, elapsed.as_secs_f64(),
                    self.added, self.already_present, self.failed, self.skipped
                );
                
                // Log error breakdown if we have categorized errors
                if !self.error_counts.is_empty() {
                    let mut error_entries: Vec<_> = self.error_counts.iter().collect();
                    error_entries.sort_by(|a, b| b.1.cmp(a.1)); // Sort by count descending
                    
                    let error_summary: Vec<String> = error_entries
                        .iter()
                        .map(|(category, count)| format!("{}: {}", category, count))
                        .collect();
                    
                    info!("Error breakdown: {}", error_summary.join(", "));
                }
            } else {
                info!(
                    "{} completed: {} total in {:.1}s | Added: {} | Already present: {} | Failed: {} | Skipped: {}",
                    operation_name, self.total, elapsed.as_secs_f64(),
                    self.added, self.already_present, self.failed, self.skipped
                );
            }
        }
    }
}

