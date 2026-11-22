use std::collections::BTreeSet;

#[derive(Debug, Clone)]
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
    let unit = 1024_f64 * 1024.0;
    let (value, suffix) = if usage >= 1024 * 1024 * 1024 {
        (usage as f64 / (unit * 1024.0), "GB")
    } else {
        (usage as f64 / unit, "MB")
    };
    let percent = quota
        .filter(|q| *q > 0)
        .map(|q| ((usage as u128 * 100 / q as u128).min(100)) as u8);
    (format!("{value:.1} {suffix}"), percent)
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
                    for near in [false, true] {
                        let status = StorageStatus {
                            persisted,
                            usage: Some(if near { 91 } else { 90 }),
                            quota: Some(100),
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

    #[test]
    fn usage_formats_binary_units_and_optional_percent() {
        assert_eq!(format_usage(1_258_291, None), ("1.2 MB".into(), None));
        assert_eq!(
            format_usage(1_288_490_189, Some(2_147_483_648)),
            ("1.2 GB".into(), Some(60))
        );
        assert_eq!(format_usage(1, Some(0)).1, None);
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
