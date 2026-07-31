use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    MissingField(&'static str),
    StopNotFound(String),
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreError::MissingField(field) => write!(f, "No {field}"),
            CoreError::StopNotFound(uic) => write!(f, "Stop {uic} not found in formation"),
        }
    }
}

impl std::error::Error for CoreError {}
