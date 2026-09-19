//! Semantic-version comparison for release tags.

/// Parse `v0.4.12` / `0.4.12` into `(major, minor, patch)`. Anything after
/// the third component (`-rc1`, `+build`) is ignored; missing trailing
/// components read as zero.
#[must_use]
pub(crate) fn parse_semver(text: &str) -> Option<(u64, u64, u64)> {
    let trimmed = text.trim().trim_start_matches(['v', 'V']);
    let core = trimmed.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse::<u64>().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse::<u64>().ok())?;
    Some((major, minor, patch))
}

/// `true` when `candidate` is strictly newer than `current`. Unparseable
/// input is never "newer" so a malformed tag can never trigger an offer.
#[must_use]
pub(crate) fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_semver(candidate), parse_semver(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_newer, parse_semver};

    #[test]
    fn parses_tags_with_and_without_prefix() {
        assert_eq!(parse_semver("v0.4.12"), Some((0, 4, 12)));
        assert_eq!(parse_semver("0.4.12"), Some((0, 4, 12)));
        assert_eq!(parse_semver("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_semver("1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_semver("sdk-v0.2.40"), None);
        assert_eq!(parse_semver(""), None);
    }

    #[test]
    fn newer_is_strict_and_component_wise() {
        assert!(is_newer("0.4.12", "0.4.11"));
        assert!(is_newer("0.5.0", "0.4.99"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.4.11", "0.4.11"));
        assert!(!is_newer("0.4.10", "0.4.11"));
        assert!(!is_newer("garbage", "0.4.11"));
    }
}
