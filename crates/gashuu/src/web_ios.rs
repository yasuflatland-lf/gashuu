/// True on iPhone, iPod, iPad, and iPadOS 13+ Safari (which reports a Macintosh user agent but has multi-touch).
pub fn is_ios_family(user_agent: &str, max_touch_points: u32) -> bool {
    user_agent.contains("iPhone")
        || user_agent.contains("iPad")
        || user_agent.contains("iPod")
        || (user_agent.contains("Macintosh") && max_touch_points > 1)
}

#[cfg(test)]
mod tests {
    use super::is_ios_family;

    #[test]
    fn identifies_ios_devices() {
        assert!(is_ios_family(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)",
            0
        ));
        assert!(is_ios_family(
            "Mozilla/5.0 (iPad; CPU OS 12_0 like Mac OS X)",
            0
        ));
        assert!(is_ios_family(
            "Mozilla/5.0 (iPod touch; CPU iPhone OS 15_0 like Mac OS X)",
            0
        ));
        assert!(is_ios_family(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) Safari/605.1.15",
            5
        ));
    }

    #[test]
    fn excludes_other_platforms() {
        assert!(!is_ios_family(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) Safari/605.1.15",
            0
        ));
        assert!(!is_ios_family(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/120.0",
            0
        ));
        assert!(!is_ios_family(
            "Mozilla/5.0 (Linux; Android 13) Chrome/120.0",
            5
        ));
        assert!(!is_ios_family(
            "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Firefox/120.0",
            0
        ));
        assert!(!is_ios_family(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)",
            1
        ));
    }
}
