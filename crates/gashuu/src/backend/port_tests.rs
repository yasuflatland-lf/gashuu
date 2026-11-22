use super::*;

#[test]
fn backend_error_maps_core_variants_and_preserves_other_detail() {
    let core = CoreError::EmptyBook {
        label: "book".into(),
    };
    assert_eq!(BackendError::from_core(&core), BackendError::EmptyBook);
    let core = CoreError::FormatDisabled { format: "RAR" };
    let disabled = BackendError::from_core(&core);
    assert_eq!(disabled, BackendError::FormatDisabled { format: "RAR" });
    assert_eq!(disabled.to_string(), core.to_string());
    let core = CoreError::Io(std::io::Error::other("bad read"));
    assert_eq!(
        BackendError::from_core(&core),
        BackendError::Other {
            detail: core.to_string()
        }
    );
}

impl OpenedBook {
    /// Read a real fixture synchronously. Only the metadata escapes this backend.
    pub(crate) fn open_for_test(
        id: &BookId,
        policy: ArchivePolicy,
        cache: CacheConfig,
    ) -> Result<Self, BackendError> {
        let backend = desktop::DesktopBackend::new();
        let mut opened = backend.open_blocking(id, policy, cache)?;
        backend.close(opened.session);
        // The fixture's fake backend owns a distinct handle for the installed metadata.
        opened.session = SessionId::allocate();
        Ok(opened)
    }
}
