//! Typed errors for gashuu-core. The presentation layer formats these with
//! color-eyre; this crate never depends on slint or eyre.

/// Errors produced by gashuu-core I/O and decoding.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Filesystem read/walk failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A page index was requested outside the available range.
    #[error("page index {index} out of range (len {len})")]
    IndexOutOfRange { index: usize, len: usize },

    /// The `image` crate failed to decode the bytes.
    #[error("image decode error: {0}")]
    Decode(#[from] image::ImageError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_out_of_range_displays_index_and_len() {
        let err = CoreError::IndexOutOfRange { index: 5, len: 3 };
        assert_eq!(err.to_string(), "page index 5 out of range (len 3)");
    }
}
