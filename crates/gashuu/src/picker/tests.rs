use super::web::{accept_filter, supported_name};

#[test]
fn ios_family_omits_accept_and_other_platforms_filter_archives() {
    assert_eq!(
        accept_filter("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0)", 5),
        None
    );
    assert_eq!(accept_filter("Mozilla/5.0 (iPad; CPU OS 18_0)", 5), None);
    assert_eq!(
        accept_filter("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)", 5),
        None
    );
    assert_eq!(
        accept_filter("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)", 0),
        Some(".cbz,.zip")
    );
    assert_eq!(
        accept_filter("Mozilla/5.0 (Linux; Android 15)", 5),
        Some(".cbz,.zip")
    );
}

#[test]
fn accepts_only_cbz_and_zip_regardless_of_case() {
    for name in ["book.cbz", "book.ZIP", "本.CbZ"] {
        assert!(supported_name(name), "{name}");
    }
    for name in [
        "book.cbr", "book.RAR", "book.7z", "book.cb7", "book.pdf", "book",
    ] {
        assert!(!supported_name(name), "{name}");
    }
}
