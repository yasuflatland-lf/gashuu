//! Pure (Slint-free) builder for the page-counter chip text, shared by `refresh`
//! and `on_scrub_preview` so the "X / N" vs "X–Y / N" rule lives in ONE place.

/// Format the page-counter chip text from a spread's leading page, its optional
/// trailing page (both 0-based), and the total page count. `"0 / 0"` when empty,
/// `"X / N"` for a single page, `"X–Y / N"` for a two-page spread (1-based display).
pub fn page_counter_text(lead: usize, trailing: Option<usize>, total: usize) -> String {
    if total == 0 {
        return "0 / 0".to_string();
    }
    match trailing {
        Some(t) => format!("{}\u{2013}{} / {}", lead + 1, t + 1, total),
        None => format!("{} / {}", lead + 1, total),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero_of_zero() {
        assert_eq!(page_counter_text(0, None, 0), "0 / 0");
    }

    #[test]
    fn single_page() {
        assert_eq!(page_counter_text(2, None, 10), "3 / 10");
    }

    #[test]
    fn double_spread() {
        assert_eq!(page_counter_text(1, Some(2), 10), "2\u{2013}3 / 10");
    }

    #[test]
    fn last_page_clamp_single() {
        // The scrubber clamps the leading page to total-1 on the last page.
        assert_eq!(page_counter_text(9, None, 10), "10 / 10");
    }
}
