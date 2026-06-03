//! Enum <-> ComboBox index conversions for the settings dialog.
//!
//! `*_to_index` use exhaustive matches so a new enum variant becomes a compile
//! error; `index_to_*` default to the first variant for any out-of-range index
//! Slint may send (the index is a raw i32). The ordering is authoritative and
//! MUST match the ComboBox model order in SettingsDialog.slint:
//!   ReadingDirection: Ltr=0, Rtl=1
//!   SpreadMode:       Single=0, Double=1, Auto=2
//!   CoverMode:        Standalone=0, Paired=1
//!   FitMode:          Whole=0, Width=1, Actual=2
//!   Language:         En=0, Ja=1

use gashuu_core::{CoverMode, FitMode, Language, ReadingDirection, SpreadMode};

pub(crate) fn reading_direction_to_index(d: ReadingDirection) -> i32 {
    match d {
        ReadingDirection::Ltr => 0,
        ReadingDirection::Rtl => 1,
    }
}

pub(crate) fn index_to_reading_direction(i: i32) -> ReadingDirection {
    match i {
        1 => ReadingDirection::Rtl,
        _ => ReadingDirection::Ltr,
    }
}

pub(crate) fn spread_mode_to_index(m: SpreadMode) -> i32 {
    match m {
        SpreadMode::Single => 0,
        SpreadMode::Double => 1,
        SpreadMode::Auto => 2,
    }
}

pub(crate) fn index_to_spread_mode(i: i32) -> SpreadMode {
    match i {
        1 => SpreadMode::Double,
        2 => SpreadMode::Auto,
        _ => SpreadMode::Single,
    }
}

pub(crate) fn cover_mode_to_index(m: CoverMode) -> i32 {
    match m {
        CoverMode::Standalone => 0,
        CoverMode::Paired => 1,
    }
}

pub(crate) fn index_to_cover_mode(i: i32) -> CoverMode {
    match i {
        1 => CoverMode::Paired,
        _ => CoverMode::Standalone,
    }
}

pub(crate) fn fit_mode_to_index(m: FitMode) -> i32 {
    match m {
        FitMode::Whole => 0,
        FitMode::Width => 1,
        FitMode::Actual => 2,
    }
}

pub(crate) fn index_to_fit_mode(i: i32) -> FitMode {
    match i {
        1 => FitMode::Width,
        2 => FitMode::Actual,
        _ => FitMode::Whole,
    }
}

pub(crate) fn language_to_index(l: Language) -> i32 {
    match l {
        Language::En => 0,
        Language::Ja => 1,
    }
}

pub(crate) fn index_to_language(i: i32) -> Language {
    match i {
        1 => Language::Ja,
        _ => Language::En,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_direction_index_round_trips() {
        for d in [ReadingDirection::Ltr, ReadingDirection::Rtl] {
            assert_eq!(index_to_reading_direction(reading_direction_to_index(d)), d);
        }
    }

    #[test]
    fn spread_mode_index_round_trips() {
        for m in [SpreadMode::Single, SpreadMode::Double, SpreadMode::Auto] {
            assert_eq!(index_to_spread_mode(spread_mode_to_index(m)), m);
        }
    }

    #[test]
    fn cover_mode_index_round_trips() {
        for m in [CoverMode::Standalone, CoverMode::Paired] {
            assert_eq!(index_to_cover_mode(cover_mode_to_index(m)), m);
        }
    }

    #[test]
    fn fit_mode_index_round_trips() {
        for m in [FitMode::Whole, FitMode::Width, FitMode::Actual] {
            assert_eq!(index_to_fit_mode(fit_mode_to_index(m)), m);
        }
    }

    #[test]
    fn language_index_round_trips() {
        for l in [Language::En, Language::Ja] {
            assert_eq!(index_to_language(language_to_index(l)), l);
        }
    }

    #[test]
    fn out_of_range_indices_clamp_to_first_variant() {
        for bad in [-1, 3, 99, i32::MIN, i32::MAX] {
            assert_eq!(index_to_reading_direction(bad), ReadingDirection::Ltr);
            assert_eq!(index_to_spread_mode(bad), SpreadMode::Single);
            assert_eq!(index_to_cover_mode(bad), CoverMode::Standalone);
            assert_eq!(index_to_fit_mode(bad), FitMode::Whole);
            assert_eq!(index_to_language(bad), Language::En);
        }
    }
}
