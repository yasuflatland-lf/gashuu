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
