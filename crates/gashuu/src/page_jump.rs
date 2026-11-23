//! Pure parser for the viewer page-jump field: maps a 1-based string input to a 0-based page index.

/// Parse a 1-based page-jump string into a 0-based page index.
///
/// ASCII and full-width digits are accepted, including mixed-width input.
///
/// Returns `None` when:
/// - `input` is empty
/// - `input` is not numeric
/// - `input` is all-whitespace (trimmed to empty before parsing)
/// - `total` is 0
///
/// For numeric input the value is clamped to `[1, total]` (treating 0 as 1),
/// then converted to a 0-based index by subtracting 1.
pub fn parse_page_jump(input: &str, total: usize) -> Option<usize> {
    if input.is_empty() {
        return None;
    }
    if total == 0 {
        return None;
    }
    let normalized = to_ascii_digits(input);
    let n: usize = normalized.trim().parse().ok()?;
    // Treat page 0 as page 1 (clamp minimum to 1).
    let clamped = n.max(1).min(total);
    Some(clamped - 1)
}

fn to_ascii_digits(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            '\u{FF10}'..='\u{FF19}' => char::from(b'0' + (c as u32 - 0xFF10) as u8),
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_page_within_range() {
        assert_eq!(parse_page_jump("5", 10), Some(4));
    }

    #[test]
    fn clamp_above_total() {
        assert_eq!(parse_page_jump("15", 10), Some(9));
    }

    #[test]
    fn clamp_below_zero_treated_as_page_one() {
        assert_eq!(parse_page_jump("0", 10), Some(0));
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(parse_page_jump("", 10), None);
    }

    #[test]
    fn non_numeric_input_returns_none() {
        assert_eq!(parse_page_jump("abc", 10), None);
    }

    #[test]
    fn total_zero_returns_none() {
        assert_eq!(parse_page_jump("1", 0), None);
    }

    #[test]
    fn first_page() {
        assert_eq!(parse_page_jump("1", 1), Some(0));
    }

    #[test]
    fn last_page() {
        assert_eq!(parse_page_jump("10", 10), Some(9));
    }

    #[test]
    fn whitespace_trimmed_numeric() {
        assert_eq!(parse_page_jump(" 3 ", 10), Some(2));
    }

    #[test]
    fn single_page_book_clamps_correctly() {
        assert_eq!(parse_page_jump("99", 1), Some(0));
    }

    #[test]
    fn negative_input_returns_none() {
        assert_eq!(parse_page_jump("-1", 10), None);
    }

    #[test]
    fn whitespace_only_returns_none() {
        assert_eq!(parse_page_jump("   ", 10), None);
    }

    #[test]
    fn full_width_digits_are_accepted() {
        assert_eq!(parse_page_jump("１", 1), Some(0));
        assert_eq!(parse_page_jump("１２", 20), Some(11));
    }

    #[test]
    fn mixed_width_digits_are_accepted() {
        assert_eq!(parse_page_jump("1２", 20), Some(11));
    }

    #[test]
    fn full_width_input_is_clamped_like_ascii() {
        assert_eq!(parse_page_jump("９９", 10), Some(9));
        assert_eq!(parse_page_jump("０", 10), Some(0));
    }

    #[test]
    fn ideographic_space_is_trimmed() {
        assert_eq!(parse_page_jump("\u{3000}３\u{3000}", 10), Some(2));
    }

    #[test]
    fn full_width_non_digits_are_rejected() {
        assert_eq!(parse_page_jump("ａｂ", 10), None);
        assert_eq!(parse_page_jump("－１", 10), None);
    }
}
