use super::{PageEntry, PageSource};
use crate::error::CoreError;

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

/// Compare two file names in natural order so embedded numbers sort by numeric
/// value (`2.png` < `10.png`). Non-digit runs compare case-insensitively (ASCII)
/// with the raw chars as a stable tiebreaker, giving a total order.
fn natural_cmp(a: &str, b: &str) -> Ordering {
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
                    match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                        Ordering::Equal => match ca.cmp(&cb) {
                            Ordering::Equal => {
                                ai.next();
                                bi.next();
                            }
                            ord => return ord,
                        },
                        ord => return ord,
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
/// leading zeros, compare by length then lexically; equal value falls back to
/// the raw runs so padding differences stay deterministic.
fn cmp_numeric(a: &str, b: &str) -> Ordering {
    let ta = a.trim_start_matches('0');
    let tb = b.trim_start_matches('0');
    match ta.len().cmp(&tb.len()) {
        Ordering::Equal => match ta.cmp(tb) {
            Ordering::Equal => a.cmp(b),
            ord => ord,
        },
        ord => ord,
    }
}

pub struct FolderSource;

impl PageSource for FolderSource {
    fn list_pages(&self) -> Vec<PageEntry> {
        unimplemented!()
    }
    fn read_bytes(&self, _index: usize) -> Result<Vec<u8>, CoreError> {
        unimplemented!()
    }
}

#[cfg(test)]
mod natural_cmp_tests {
    use super::natural_cmp;
    use std::cmp::Ordering;

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
}
