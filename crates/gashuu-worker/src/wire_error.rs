use gashuu_core::{ArchiveKind, CoreError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WireError {
    EmptyBook,
    FormatDisabled { kind: ArchiveKind },
    BookLocked,
    StorageUnavailable,
    Missing { id: String },
    Other { detail: String },
}

impl From<&CoreError> for WireError {
    fn from(error: &CoreError) -> Self {
        match error {
            CoreError::EmptyBook { .. } => Self::EmptyBook,
            CoreError::FormatDisabled { kind } => Self::FormatDisabled { kind: *kind },
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

/// Classify an OPFS DOM exception using its stable name.
pub fn classify(dom_name: &str, id: &str) -> WireError {
    match dom_name {
        "NotFoundError" => WireError::Missing { id: id.into() },
        "NoModificationAllowedError" => WireError::BookLocked,
        "SecurityError" | "NotSupportedError" => WireError::StorageUnavailable,
        "QuotaExceededError" => WireError::Other {
            detail: crate::import::STORAGE_FULL.into(),
        },
        _ => WireError::Other {
            detail: dom_name.into(),
        },
    }
}
