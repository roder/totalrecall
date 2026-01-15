use clap::ValueEnum;
use owo_colors::OwoColorize;
use serde_json::json;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
    #[value(name = "json-pretty")]
    JsonPretty,
}

impl OutputFormat {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "human" => Ok(OutputFormat::Human),
            "json" => Ok(OutputFormat::Json),
            "json-pretty" | "json_pretty" => Ok(OutputFormat::JsonPretty),
            _ => Err(format!("Invalid output format: {}. Use 'human', 'json', or 'json-pretty'", s)),
        }
    }
}

pub struct Output {
    format: OutputFormat,
    quiet: bool,
}

impl Output {
    pub fn new(format: OutputFormat, quiet: bool) -> Self {
        Self { format, quiet }
    }

    pub fn format(&self) -> OutputFormat {
        self.format
    }

    pub fn is_quiet(&self) -> bool {
        self.quiet
    }

    pub fn success(&self, msg: impl AsRef<str>) {
        if self.quiet {
            return;
        }

        match self.format {
            OutputFormat::Human => {
                println!("{} {}", "✓".green(), msg.as_ref());
            }
            OutputFormat::Json | OutputFormat::JsonPretty => {
                let json = json!({
                    "type": "success",
                    "message": msg.as_ref()
                });
                self.print_json(&json);
            }
        }
    }

    pub fn error(&self, msg: impl AsRef<str>) {
        // Errors should always be shown, even in quiet mode
        match self.format {
            OutputFormat::Human => {
                eprintln!("{} {}", "✗".red(), msg.as_ref());
            }
            OutputFormat::Json | OutputFormat::JsonPretty => {
                let json = json!({
                    "type": "error",
                    "message": msg.as_ref()
                });
                self.print_json(&json);
            }
        }
    }

    pub fn info(&self, msg: impl AsRef<str>) {
        if self.quiet {
            return;
        }

        match self.format {
            OutputFormat::Human => {
                println!("{}", msg.as_ref());
            }
            OutputFormat::Json | OutputFormat::JsonPretty => {
                let json = json!({
                    "type": "info",
                    "message": msg.as_ref()
                });
                self.print_json(&json);
            }
        }
    }

    pub fn warn(&self, msg: impl AsRef<str>) {
        if self.quiet {
            return;
        }

        match self.format {
            OutputFormat::Human => {
                println!("{} {}", "⚠".yellow(), msg.as_ref());
            }
            OutputFormat::Json | OutputFormat::JsonPretty => {
                let json = json!({
                    "type": "warning",
                    "message": msg.as_ref()
                });
                self.print_json(&json);
            }
        }
    }

    pub fn println(&self, msg: impl AsRef<str>) {
        if self.quiet {
            return;
        }

        match self.format {
            OutputFormat::Human => {
                println!("{}", msg.as_ref());
            }
            OutputFormat::Json | OutputFormat::JsonPretty => {
                // For plain println in JSON mode, output as info
                let json = json!({
                    "type": "info",
                    "message": msg.as_ref()
                });
                self.print_json(&json);
            }
        }
    }

    pub fn eprintln(&self, msg: impl AsRef<str>) {
        // Errors should always be shown
        match self.format {
            OutputFormat::Human => {
                eprintln!("{}", msg.as_ref());
            }
            OutputFormat::Json | OutputFormat::JsonPretty => {
                let json = json!({
                    "type": "error",
                    "message": msg.as_ref()
                });
                self.print_json(&json);
            }
        }
    }

    pub fn json(&self, data: &serde_json::Value) {
        if self.quiet && self.format != OutputFormat::Human {
            return;
        }

        self.print_json(data);
    }

    fn print_json(&self, data: &serde_json::Value) {
        match self.format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string(data).unwrap_or_default());
            }
            OutputFormat::JsonPretty => {
                println!("{}", serde_json::to_string_pretty(data).unwrap_or_default());
            }
            OutputFormat::Human => {
                // Shouldn't happen, but fallback to string representation
                println!("{}", data);
            }
        }
    }

    pub fn print(&self, msg: impl AsRef<str>) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }

        match self.format {
            OutputFormat::Human => {
                print!("{}", msg.as_ref());
                io::stdout().flush()?;
            }
            OutputFormat::Json | OutputFormat::JsonPretty => {
                // For print! in JSON mode, we typically don't want to output yet
                // This is mainly for prompts, so we'll still print in human format
                print!("{}", msg.as_ref());
                io::stdout().flush()?;
            }
        }
        Ok(())
    }
}

