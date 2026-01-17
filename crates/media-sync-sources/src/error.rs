use std::fmt;

#[derive(Debug)]
pub struct SourceError {
    message: String,
}

impl SourceError {
    pub fn new(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SourceError {}


