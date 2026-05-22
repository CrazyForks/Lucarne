use semver::Version;

pub fn normalize_tag(tag: &str) -> &str {
    let trimmed = tag.trim();
    trimmed.strip_prefix('v').unwrap_or(trimmed)
}

pub fn parse_version(value: &str) -> Result<Version, semver::Error> {
    Version::parse(normalize_tag(value))
}

pub fn is_newer_version(current: &str, latest_tag: &str) -> Result<bool, semver::Error> {
    Ok(parse_version(latest_tag)? > parse_version(current)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_single_v_prefix_and_compares_versions() {
        assert_eq!(normalize_tag("v0.2.0"), "0.2.0");
        assert_eq!(normalize_tag("0.2.0"), "0.2.0");
        assert!(is_newer_version("0.1.0", "v0.2.0").unwrap());
        assert!(!is_newer_version("0.2.0", "v0.2.0").unwrap());
        assert!(!is_newer_version("0.3.0", "v0.2.0").unwrap());
    }

    #[test]
    fn invalid_latest_version_is_an_error() {
        assert!(is_newer_version("0.1.0", "nightly").is_err());
    }
}
