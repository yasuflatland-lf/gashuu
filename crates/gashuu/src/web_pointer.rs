/// Whether a single tap on the centered cover opens the book.
pub fn taps_open_centered(pointer_is_coarse: bool) -> bool {
    pointer_is_coarse
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn apply_pointer_mode(ui: &crate::ViewerWindow) {
    let coarse =
        web_sys::window().is_some_and(|window| match window.match_media("(pointer: coarse)") {
            Ok(query) => query.is_some_and(|query| query.matches()),
            Err(error) => {
                tracing::warn!(?error, "could not query browser pointer type");
                false
            }
        });
    ui.set_tap_opens_centered(taps_open_centered(coarse));
}

#[cfg(test)]
mod tests {
    use super::taps_open_centered;

    #[test]
    fn coarse_pointer_opens_on_tap() {
        assert!(taps_open_centered(true));
    }

    #[test]
    fn fine_pointer_requires_explicit_open() {
        assert!(!taps_open_centered(false));
    }
}
