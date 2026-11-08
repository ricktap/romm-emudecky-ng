//! RomM version string parsing and minimum-version comparison.

/// The oldest RomM release the plugin talks to (Device Sync / `negotiate`).
pub const MIN_REQUIRED_VERSION: &[u64] = &[4, 9, 0];

/// Parse `version` into `(numeric core, has_prerelease)`.
///
/// Accepts `N(.N)*` optionally followed by `-alpha` / `-beta` (any case) with an
/// optional `.N` build number. Anything else is `None`.
fn parse(version: &str) -> Option<(Vec<u64>, bool)> {
    let (core, suffix) = match version.split_once('-') {
        Some((core, suffix)) => (core, Some(suffix)),
        None => (version, None),
    };
    let parts: Option<Vec<u64>> = core
        .split('.')
        .map(|p| {
            if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
                p.parse().ok()
            } else {
                None
            }
        })
        .collect();
    let parts = parts?;
    if let Some(suffix) = suffix {
        let lower = suffix.to_ascii_lowercase();
        let (tag, build) = match lower.split_once('.') {
            Some((tag, build)) => (tag.to_string(), Some(build.to_string())),
            None => (lower, None),
        };
        if tag != "alpha" && tag != "beta" {
            return None;
        }
        if let Some(build) = build
            && (build.is_empty() || !build.chars().all(|c| c.is_ascii_digit()))
        {
            return None;
        }
    }
    Some((parts, suffix.is_some()))
}

/// True when `version` is `>= minimum`; a pre-release ranks below its own
/// release. Unparseable input (including `"development"`) is `false`.
pub fn meets_min_version(version: &str, minimum: &[u64]) -> bool {
    let Some((core, prerelease)) = parse(version) else {
        return false;
    };
    match core.as_slice().cmp(minimum) {
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => !prerelease,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &[u64] = &[4, 8, 1];

    #[test]
    fn releases() {
        assert!(meets_min_version("4.8.1", MIN));
        assert!(meets_min_version("4.8.2", MIN));
        assert!(meets_min_version("4.9.0", MIN));
        assert!(meets_min_version("5.0.0", MIN));
        assert!(!meets_min_version("4.8.0", MIN));
        assert!(!meets_min_version("4.7.0", MIN));
        assert!(!meets_min_version("3.99.99", MIN));
        assert!(!meets_min_version("4.8", MIN));
        assert!(meets_min_version("4.8.1.1", MIN));
    }

    #[test]
    fn garbage() {
        for bad in [
            "abc",
            "",
            "development",
            "4.8.x",
            "5.0.0-alpha.",
            "5.0.0-rc.1",
            "4..1",
            "-beta",
        ] {
            assert!(!meets_min_version(bad, MIN), "{bad}");
        }
    }

    #[test]
    fn prereleases() {
        assert!(meets_min_version("5.0.0-alpha.1", MIN));
        assert!(meets_min_version("5.0.0-alpha", MIN));
        assert!(!meets_min_version("4.8.1-beta.3", MIN));
        assert!(!meets_min_version("4.8.1-alpha.1", MIN));
        assert!(!meets_min_version("4.8.1-beta", MIN));
        assert!(meets_min_version("4.8.2-beta", MIN));
        assert!(meets_min_version("4.8.2-BETA", MIN));
        assert!(meets_min_version("5.0.0-ALPHA.1", MIN));
        assert!(!meets_min_version("4.8.0-alpha.99", MIN));
    }

    #[test]
    fn current_floor() {
        assert!(meets_min_version("4.9.0", MIN_REQUIRED_VERSION));
        assert!(!meets_min_version("4.8.1", MIN_REQUIRED_VERSION));
        assert!(!meets_min_version("4.9.0-beta.3", MIN_REQUIRED_VERSION));
        assert!(meets_min_version("4.9.1-beta", MIN_REQUIRED_VERSION));
    }
}
