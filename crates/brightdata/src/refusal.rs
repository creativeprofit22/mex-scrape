//! Typed classification of Bright Data access refusals that need KYC.
//!
//! Pure functions: no I/O. See
//! <https://docs.brightdata.com/products/web-unlocker/kyc-and-access>.

/// Why Bright Data refused a target until KYC is approved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// Web Unlocker 400 `ub_bad_endpoint_robots`: robots.txt disallows it.
    RobotsDisallowed,
    /// Web Unlocker 403 with a `policy_` code: compliance approval needed.
    Policy,
    /// Browser API `brob` navigation error (robots.txt).
    BrowserRobots,
}

impl RefusalReason {
    /// Fixed, URL-free description.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::RobotsDisallowed => "endpoint disallowed by the target's robots.txt",
            Self::Policy => "target requires compliance approval",
            Self::BrowserRobots => "Browser API refused navigation (brob)",
        }
    }
}

/// Classifies a Web Unlocker response by HTTP status and `X-Brd-Error-Code`.
/// Plain 400/403 without those codes are not KYC refusals.
#[must_use]
pub fn classify_unlocker(status: u16, error_code: &str) -> Option<RefusalReason> {
    let code = error_code.trim().to_ascii_lowercase();
    match status {
        400 if code == "ub_bad_endpoint_robots" => Some(RefusalReason::RobotsDisallowed),
        403 if code.starts_with("policy_") => Some(RefusalReason::Policy),
        _ => None,
    }
}

/// Classifies a Browser API (`page.goto`) error message: the `brob`
/// protocol error means the endpoint is robots-disallowed.
#[must_use]
pub fn classify_browser_error(message: &str) -> Option<RefusalReason> {
    message
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| word.eq_ignore_ascii_case("brob"))
        .then_some(RefusalReason::BrowserRobots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlocker_codes_map() {
        assert_eq!(
            classify_unlocker(400, "ub_bad_endpoint_robots"),
            Some(RefusalReason::RobotsDisallowed)
        );
        assert_eq!(
            classify_unlocker(403, "policy_20050"),
            Some(RefusalReason::Policy)
        );
    }

    #[test]
    fn plain_errors_are_not_refusals() {
        assert_eq!(classify_unlocker(400, ""), None);
        assert_eq!(classify_unlocker(403, ""), None);
        assert_eq!(classify_unlocker(403, "ub_bad_endpoint_robots"), None);
        assert_eq!(classify_unlocker(400, "policy_1"), None);
        assert_eq!(classify_unlocker(502, "policy_1"), None);
    }

    #[test]
    fn browser_brob_maps() {
        assert_eq!(
            classify_browser_error("Protocol error (Page.navigate): brob"),
            Some(RefusalReason::BrowserRobots)
        );
        assert_eq!(classify_browser_error("net::ERR_TIMED_OUT"), None);
        assert_eq!(classify_browser_error("a brobdingnagian error"), None);
    }
}
