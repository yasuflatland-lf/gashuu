use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StorageStatus {
    pub persisted: Option<bool>,
    pub usage: Option<u64>,
    pub quota: Option<u64>,
    pub standalone: bool,
    pub ios_family: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Guidance {
    None,
    AddToHomeScreen,
    PersistDenied,
    NearQuota,
    Evicted { count: usize },
}

impl Guidance {
    pub(crate) fn kind(&self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::AddToHomeScreen => Some("add-to-home-screen"),
            Self::PersistDenied => Some("persist-denied"),
            Self::NearQuota => Some("near-quota"),
            Self::Evicted { .. } => Some("evicted"),
        }
    }
}

pub(crate) fn guidance(s: &StorageStatus, missing_books: usize) -> Guidance {
    if missing_books > 0 {
        Guidance::Evicted {
            count: missing_books,
        }
    } else if matches!((s.usage, s.quota), (Some(u), Some(q)) if q > 0 && u as u128 * 10 > q as u128 * 9)
    {
        Guidance::NearQuota
    } else if s.persisted == Some(false) && s.ios_family && !s.standalone {
        Guidance::AddToHomeScreen
    } else if s.persisted == Some(false) {
        Guidance::PersistDenied
    } else {
        Guidance::None
    }
}

pub(crate) fn format_usage(usage: u64, quota: Option<u64>) -> (String, Option<u8>) {
    let percent = quota.filter(|&quota| quota > 0).map(|quota| {
        let quota = u128::from(quota);
        let rounded = (u128::from(usage) * 100 + quota / 2) / quota;
        rounded.min(100) as u8
    });
    (format_bytes(usage), percent)
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    let (value, suffix) = if mib >= 1024.0 - 0.05 {
        (mib / 1024.0, "GB")
    } else {
        (mib, "MB")
    };
    format!("{value:.1} {suffix}")
}

pub(crate) fn notice_visible(guidance: &Guidance, dismissed: &mut BTreeSet<String>) -> bool {
    let kind = guidance.kind();
    if dismissed.iter().any(|value| Some(value.as_str()) != kind) {
        dismissed.clear();
    }
    kind.is_some_and(|kind| !dismissed.contains(kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guidance_prioritizes_missing_then_quota_then_platform() {
        for persisted in [None, Some(false), Some(true)] {
            for ios_family in [false, true] {
                for standalone in [false, true] {
                    for (usage, quota, near) in [
                        (Some(90), Some(100), false),
                        (Some(91), Some(100), true),
                        (None, Some(100), false),
                        (Some(u64::MAX), None, false),
                        (Some(u64::MAX), Some(0), false),
                        (Some(u64::MAX / 10 * 9), Some(u64::MAX), false),
                    ] {
                        let status = StorageStatus {
                            persisted,
                            usage,
                            quota,
                            standalone,
                            ios_family,
                        };
                        assert_eq!(guidance(&status, 2), Guidance::Evicted { count: 2 });
                        let expected = if near {
                            Guidance::NearQuota
                        } else if persisted == Some(false) && ios_family && !standalone {
                            Guidance::AddToHomeScreen
                        } else if persisted == Some(false) {
                            Guidance::PersistDenied
                        } else {
                            Guidance::None
                        };
                        assert_eq!(guidance(&status, 0), expected);
                    }
                }
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn usage_formats_bounded_percent_and_promotes_rounded_units(
            bytes in proptest::prop_oneof![
                0u64..(4 << 30),
                ((1u64 << 30) - 60_000)..=((1u64 << 30) + 60_000),
                proptest::strategy::Just(u64::MAX),
            ],
            quota in proptest::prop_oneof![
                proptest::strategy::Just(None),
                proptest::strategy::Just(Some(0)),
                proptest::option::of(proptest::num::u64::ANY),
                proptest::option::of(1u64..(4 << 30)),
            ],
        ) {
            let (display, percent) = format_usage(bytes, quota);
            proptest::prop_assert_eq!(&display, &format_bytes(bytes));
            let (number, unit) = display.split_once(' ').unwrap();
            let number: f64 = number.parse().unwrap();
            match unit {
                "MB" => proptest::prop_assert!((0.0..1024.0).contains(&number), "{}", display),
                // GB is the largest supported unit, including for u64::MAX.
                "GB" => {
                    let gibibytes = bytes as f64 / (1u64 << 30) as f64;
                    proptest::prop_assert!((number - gibibytes).abs() <= 0.051, "{}", display);
                }
                _ => proptest::prop_assert!(false, "unexpected unit in {}", display),
            }
            proptest::prop_assert_eq!(percent.is_some(), quota.is_some_and(|q| q > 0));
            if let Some(percent) = percent {
                proptest::prop_assert!(percent <= 100);
            }
        }
    }

    #[test]
    fn usage_formats_binary_units() {
        assert_eq!(format_usage(0, None), ("0.0 MB".into(), None));
        assert_eq!(format_usage(1_258_291, None), ("1.2 MB".into(), None));
        assert_eq!(format_usage(1 << 30, None), ("1.0 GB".into(), None));
        assert_eq!(format_usage(1_288_490_189, None), ("1.2 GB".into(), None));
    }

    #[test]
    fn usage_rounding_promotes_to_gigabytes() {
        assert_eq!(format_usage(1_073_678_909, None).0, "1023.9 MB"); // 1023.94 MiB
        assert_eq!(format_usage(1_073_699_881, None).0, "1.0 GB"); // 1023.96 MiB
    }

    #[test]
    fn usage_percent_requires_a_positive_quota() {
        assert_eq!(format_usage(1, None).1, None);
        assert_eq!(format_usage(1, Some(0)).1, None);
        assert_eq!(format_usage(0, Some(100)).1, Some(0));
    }

    #[test]
    fn usage_percent_rounds_to_the_nearest_integer() {
        assert_eq!(format_usage(1, Some(3)).1, Some(33));
        assert_eq!(format_usage(2, Some(3)).1, Some(67));
        assert_eq!(format_usage(1, Some(8)).1, Some(13));
    }

    #[test]
    fn usage_percent_clamps_at_100_without_overflow() {
        assert_eq!(format_usage(u64::MAX, Some(1)).1, Some(100));
        assert_eq!(format_usage(u64::MAX, Some(u64::MAX)).1, Some(100));
    }

    #[test]
    fn dismissal_is_reset_when_kind_changes() {
        let mut dismissed = Default::default();
        assert!(notice_visible(&Guidance::NearQuota, &mut dismissed));
        dismissed.insert("near-quota".to_string());
        assert!(!notice_visible(&Guidance::NearQuota, &mut dismissed));
        assert!(notice_visible(&Guidance::PersistDenied, &mut dismissed));
        assert!(dismissed.is_empty());
    }
}
