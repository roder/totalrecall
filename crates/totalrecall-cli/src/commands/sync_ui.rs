use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::io::IsTerminal;

pub struct SyncUI {
    multi: MultiProgress,
    overall: ProgressBar,
    source_bars: HashMap<String, ProgressBar>,
    spinner: ProgressBar,
    interactive: bool,
}

impl SyncUI {
    pub fn new() -> Self {
        let interactive = is_interactive();
        let multi = MultiProgress::new();

        let overall = multi.add(ProgressBar::new(100));
        overall.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({percent}%) {msg}")
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏  ")
        );
        overall.set_message("Starting sync...");

        let spinner = multi.add(ProgressBar::new_spinner());
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
        );

        if !interactive {
            tracing::info!(
                operation = "ui_init",
                mode = "non_interactive",
                "Running in non-interactive mode - progress bars disabled, using structured logging"
            );
        }

        Self {
            multi,
            overall,
            source_bars: HashMap::new(),
            spinner,
            interactive,
        }
    }

    pub fn add_source(&mut self, name: &str) -> ProgressBar {
        let pb = self.multi.add(ProgressBar::new(100));
        pb.set_style(
            ProgressStyle::default_bar()
                .template(&format!("  {{spinner:.yellow}} [{{elapsed_precise}}] [{{wide_bar:.yellow/blue}}] {{pos}}/{{len}} {{msg}}"))
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏  ")
        );
        pb.set_message(format!("{}: Initializing...", name));
        self.source_bars.insert(name.to_string(), pb.clone());
        pb
    }

    pub fn set_spinner_message(&self, msg: String) {
        if self.interactive {
            self.spinner.set_message(msg);
        } else {
            tracing::info!(operation = "progress", message = %msg, "Progress update");
        }
    }

    pub fn finish_spinner(&self) {
        if self.interactive {
            self.spinner.finish_with_message("Done");
        }
    }

    pub fn should_show_progress(&self) -> bool {
        self.interactive
    }

    pub fn set_progress(&self, current: u64, total: u64, message: &str) {
        if self.should_show_progress() {
            // Progress bars will be updated individually
        } else {
            tracing::info!(
                operation = "progress",
                current = current,
                total = total,
                percent = (current as f64 / total as f64 * 100.0) as u8,
                message = message,
                "Sync progress update"
            );
        }
    }

    pub fn overall(&self) -> &ProgressBar {
        &self.overall
    }

    pub fn source_bar(&self, name: &str) -> Option<&ProgressBar> {
        self.source_bars.get(name)
    }
}

pub fn is_interactive() -> bool {
    std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
}

