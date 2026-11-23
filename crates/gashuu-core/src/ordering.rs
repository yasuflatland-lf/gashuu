//! Shared numeric-aware natural-ordering comparator.
//!
//! Provides [`natural_cmp`], which sorts strings so embedded numbers are
//! compared by numeric value (`vol 1 < vol 2 < vol 10`) and other characters
//! compare case-insensitively in ASCII. Equal natural keys use the raw strings
//! as a final tie-break. Used by page-source and `Library` ordering.

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

/// Compare two file names in natural order so embedded numbers sort by numeric
/// value (`2.png` < `10.png`). Other characters compare case-insensitively in
/// ASCII; non-ASCII characters compare raw. Equal natural keys use the complete
/// raw strings as a final tie-break, giving a total order.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    natural_key_cmp(a, b).then_with(|| a.cmp(b))
}

/// Compare natural keys without resolving case or padding differences.
fn natural_key_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let run_a = take_digits(&mut ai);
                    let run_b = take_digits(&mut bi);
                    match cmp_numeric(&run_a, &run_b) {
                        Ordering::Equal => continue,
                        ord => return ord,
                    }
                } else {
                    let ord = ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase());
                    match ord {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

/// Consume and return the maximal leading run of ASCII digits.
fn take_digits(it: &mut Peekable<Chars<'_>>) -> String {
    let mut run = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            run.push(c);
            it.next();
        } else {
            break;
        }
    }
    run
}

/// Compare two digit runs by numeric value without integer overflow: strip
/// leading zeros, then compare by length and lexically.
fn cmp_numeric(a: &str, b: &str) -> Ordering {
    let ta = a.trim_start_matches('0');
    let tb = b.trim_start_matches('0');
    ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb))
}

#[cfg(test)]
mod natural_cmp_tests {
    use super::natural_cmp;
    use std::cmp::Ordering;

    #[test]
    fn case_does_not_decide_before_numbers() {
        assert_eq!(natural_cmp("vol 2", "Vol 10"), Ordering::Less);
        assert_eq!(natural_cmp("Vol 10", "vol 2"), Ordering::Greater);
    }

    #[test]
    fn mixed_case_volumes_sort_numerically() {
        let mut names = ["Berserk 10", "berserk 2", "Berserk 1"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["Berserk 1", "berserk 2", "Berserk 10"]);
    }

    #[test]
    fn case_insensitive_content_decides_before_raw_case() {
        assert_eq!(natural_cmp("AA", "a0"), Ordering::Greater);
    }

    #[test]
    fn padding_tie_break_runs_after_the_whole_string() {
        assert_eq!(natural_cmp("01 b", "1 a"), Ordering::Greater);
        assert_eq!(natural_cmp("001b", "1a"), Ordering::Greater);
    }

    #[test]
    fn raw_tie_break_is_total() {
        for (a, b) in [
            ("A.png", "a.png"),
            ("001.png", "1.png"),
            ("File01", "file1"),
        ] {
            assert_eq!(natural_cmp(a, b), natural_cmp(b, a).reverse());
            assert_ne!(natural_cmp(a, b), Ordering::Equal);
        }
    }

    #[test]
    fn numbers_sort_by_value_not_lexically() {
        assert_eq!(natural_cmp("2.png", "10.png"), Ordering::Less);
        assert_eq!(natural_cmp("10.png", "2.png"), Ordering::Greater);
    }

    #[test]
    fn mixed_text_and_numbers() {
        assert_eq!(natural_cmp("img1.png", "img2.png"), Ordering::Less);
        assert_eq!(natural_cmp("img2.png", "img10.png"), Ordering::Less);
    }

    #[test]
    fn case_insensitive_with_stable_tiebreak() {
        assert_eq!(natural_cmp("a.png", "B.png"), Ordering::Less);
        assert_eq!(natural_cmp("A.png", "a.png"), Ordering::Less);
    }

    #[test]
    fn equal_strings_are_equal() {
        assert_eq!(natural_cmp("005.png", "005.png"), Ordering::Equal);
    }

    #[test]
    fn same_value_different_padding_is_deterministic() {
        // Equal numeric value: more leading zeros sort first (stable, total order).
        assert_eq!(natural_cmp("001.png", "1.png"), Ordering::Less);
    }

    #[test]
    fn bare_numeric_strings_sort_numerically() {
        assert_eq!(natural_cmp("2", "10"), Ordering::Less);
        assert_eq!(natural_cmp("10", "2"), Ordering::Greater);
        assert_eq!(natural_cmp("7", "7"), Ordering::Equal);
    }

    #[test]
    fn empty_strings() {
        assert_eq!(natural_cmp("", ""), Ordering::Equal);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("a", ""), Ordering::Greater);
        assert_eq!(natural_cmp("", "1"), Ordering::Less);
    }

    #[test]
    fn digit_prefix_sorts_before_letter_prefix() {
        // ASCII digits (0x30-0x39) sort below letters (0x41+), so a digit-prefixed
        // string compares less via natural_cmp's case-insensitive char comparison.
        assert_eq!(natural_cmp("1vol", "vol1"), Ordering::Less);
        assert_eq!(natural_cmp("vol1", "1vol"), Ordering::Greater);
    }
}
