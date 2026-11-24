//! Validate an OPFS-relative book id before looking up any directory.

use crate::wire_error::WireError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookPath {
    pub uuid: String,
    pub name: String,
}

pub fn parse(id: &str) -> Result<BookPath, WireError> {
    fn valid(segment: &str) -> bool {
        !segment.is_empty() && !matches!(segment, "." | "..") && !segment.contains('\\')
    }

    let mut parts = id.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("books"), Some(uuid), Some(name), None)
            if valid(uuid) && valid(name) && !name.starts_with('.') =>
        {
            Ok(BookPath {
                uuid: uuid.into(),
                name: name.into(),
            })
        }
        _ => Err(WireError::Missing { id: id.into() }),
    }
}
