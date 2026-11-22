use gashuu_core::CoreError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WireError {
    EmptyBook,
    FormatDisabled { format: String },
    Missing { detail: String },
    Other { detail: String },
}

impl From<&CoreError> for WireError {
    fn from(error: &CoreError) -> Self {
        match error {
            CoreError::EmptyBook { .. } => Self::EmptyBook,
            CoreError::FormatDisabled { format } => Self::FormatDisabled {
                format: format.to_string(),
            },
            _ => Self::Other {
                detail: error.to_string(),
            },
        }
    }
}

impl From<CoreError> for WireError {
    fn from(error: CoreError) -> Self {
        Self::from(&error)
    }
}
