//! === BUILD VERSION VALIDATION ===
//!
//! PURPOSE:
//! Provides strict SemVer 2.0.0 parsing and build-time `IMGFORGE_BUILD_VERSION`
//! resolution shared between `crates/imgforge-cli/build.rs` and unit tests.
//!
//! THREAT MODEL / DESIGN RATIONALE ("WHY"):
//! Release workflows inject `IMGFORGE_BUILD_VERSION` from Git tags (for example
//! `v1.2.3`). Failing closed on malformed strings at compile time prevents
//! broken version strings from shipping in `imgforge --version` or `/api/health`.
//!
//! EXECUTION LIFECYCLE / DATA FLOW:
//! 1. If `IMGFORGE_BUILD_VERSION` is unset or empty (`""`), fall back to
//!    `CARGO_PKG_VERSION`.
//! 2. Otherwise strip an optional single leading `'v'` or `'V'` and validate
//!    `MAJOR.MINOR.PATCH` plus optional `-pre-release` / `+build` suffixes.
//! 3. Return the normalized SemVer string or a descriptive validation error.

/// Validate that a SemVer numeric core segment (`MAJOR`, `MINOR`, or `PATCH`)
/// consists solely of ASCII digits without leading zeroes (except `"0"`).
fn is_valid_core_number(segment: &str) -> bool {
    !segment.is_empty()
        && segment.bytes().all(|b| b.is_ascii_digit())
        && !(segment.len() > 1 && segment.starts_with('0'))
}

/// Validate a dot-separated SemVer pre-release or build metadata section.
///
/// When `disallow_numeric_leading_zeros` is `true` (pre-release identifiers),
/// numeric-only identifiers must not have leading zeroes per SemVer 2.0.0.
fn is_valid_metadata_identifiers(section: &str, disallow_numeric_leading_zeros: bool) -> bool {
    if section.is_empty() {
        return false;
    }
    section.split('.').all(|ident| {
        if ident.is_empty()
            || !ident
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return false;
        }
        if disallow_numeric_leading_zeros
            && ident.len() > 1
            && ident.bytes().all(|b| b.is_ascii_digit())
            && ident.starts_with('0')
        {
            return false;
        }
        true
    })
}

/// Strip an optional leading `'v'` or `'V'` and validate strict SemVer 2.0.0
/// (`MAJOR.MINOR.PATCH[-pre-release][+build]`).
///
/// # Errors
///
/// Returns a descriptive error string when the input is empty, has malformed
/// core digits or leading zeroes, or contains invalid pre-release/build syntax.
pub fn parse_and_validate_semver(raw: &str) -> Result<String, String> {
    let candidate = raw.strip_prefix(['v', 'V']).unwrap_or(raw);
    if candidate.is_empty() {
        return Err(format!(
            "version '{raw}' is empty after stripping optional 'v'/'V' prefix"
        ));
    }

    let core_end = candidate.find(['-', '+']).unwrap_or(candidate.len());
    let (core, suffix) = candidate.split_at(core_end);

    let mut core_parts = core.split('.');
    let major = core_parts.next().unwrap_or("");
    let minor = core_parts.next().unwrap_or("");
    let patch = core_parts.next().unwrap_or("");
    if core_parts.next().is_some()
        || !is_valid_core_number(major)
        || !is_valid_core_number(minor)
        || !is_valid_core_number(patch)
    {
        return Err(format!(
            "version '{raw}' must match strict MAJOR.MINOR.PATCH (digits without leading zeroes)"
        ));
    }

    if let Some(after_dash) = suffix.strip_prefix('-') {
        let (prerelease, build_opt) = match after_dash.split_once('+') {
            Some((pre, build)) => (pre, Some(build)),
            None => (after_dash, None),
        };
        if !is_valid_metadata_identifiers(prerelease, true) {
            return Err(format!(
                "version '{raw}' has invalid SemVer pre-release suffix '-{prerelease}'"
            ));
        }
        if let Some(build) = build_opt
            && !is_valid_metadata_identifiers(build, false)
        {
            return Err(format!(
                "version '{raw}' has invalid SemVer build metadata '+{build}'"
            ));
        }
    } else if let Some(build) = suffix.strip_prefix('+')
        && !is_valid_metadata_identifiers(build, false)
    {
        return Err(format!(
            "version '{raw}' has invalid SemVer build metadata '+{build}'"
        ));
    }

    Ok(candidate.to_string())
}

/// Resolve the effective build version from `IMGFORGE_BUILD_VERSION` and
/// `CARGO_PKG_VERSION`.
///
/// Precedence:
/// 1. `raw_override == Some(v)` where `!v.is_empty()` -> validated `v` (with optional leading `v`/`V` stripped).
/// 2. `raw_override == None` or `Some("")` -> validated `cargo_pkg_version`.
///
/// # Errors
///
/// Returns an error string when the selected version is not valid strict SemVer.
pub fn resolve_build_version(
    raw_override: Option<&str>,
    cargo_pkg_version: &str,
) -> Result<String, String> {
    match raw_override {
        Some(raw) if !raw.is_empty() => parse_and_validate_semver(raw),
        _ => parse_and_validate_semver(cargo_pkg_version),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{parse_and_validate_semver, resolve_build_version};

    #[rstest]
    #[case::bare_semver("0.1.0", Some("0.1.0"))]
    #[case::lowercase_v_prefix("v1.2.3", Some("1.2.3"))]
    #[case::uppercase_v_prefix("V2.10.0", Some("2.10.0"))]
    #[case::prerelease("v1.2.3-rc.1", Some("1.2.3-rc.1"))]
    #[case::prerelease_and_build(
        "V1.2.3-alpha.1+build-007.sha",
        Some("1.2.3-alpha.1+build-007.sha")
    )]
    #[case::build_only("1.2.3+20261114", Some("1.2.3+20261114"))]
    #[case::prerelease_alphanumeric_starting_with_zero("1.2.3-01a", Some("1.2.3-01a"))]
    #[case::empty_rejected("", None)]
    #[case::bare_v_rejected("v", None)]
    #[case::bare_upper_v_rejected("V", None)]
    #[case::double_v_rejected("vv1.2.3", None)]
    #[case::missing_patch_rejected("1.2", None)]
    #[case::four_components_rejected("1.2.3.4", None)]
    #[case::leading_zero_major_rejected("01.2.3", None)]
    #[case::leading_zero_minor_rejected("1.02.3", None)]
    #[case::leading_zero_patch_rejected("1.2.03", None)]
    #[case::whitespace_rejected(" 1.2.3", None)]
    #[case::empty_prerelease_rejected("1.2.3-", None)]
    #[case::empty_build_rejected("1.2.3+", None)]
    #[case::leading_zero_numeric_prerelease_rejected("1.2.3-01", None)]
    #[case::empty_prerelease_ident_rejected("1.2.3-alpha..1", None)]
    #[case::empty_build_ident_rejected("1.2.3+build..1", None)]
    #[case::underscore_in_suffix_rejected("1.2.3_rc1", None)]
    fn parses_and_validates_semver_strictly(#[case] raw: &str, #[case] expected: Option<&str>) {
        let res = parse_and_validate_semver(raw);
        match expected {
            Some(exp) => assert_eq!(res.expect("valid SemVer"), exp),
            None => assert!(res.is_err(), "expected '{raw}' to be rejected"),
        }
    }

    #[rstest]
    #[case::unset_uses_cargo_pkg_version(None, "0.1.0", Some("0.1.0"))]
    #[case::empty_uses_cargo_pkg_version(Some(""), "0.1.0", Some("0.1.0"))]
    #[case::valid_override_wins(Some("v1.4.2-rc.1"), "0.1.0", Some("1.4.2-rc.1"))]
    #[case::whitespace_only_override_fails_closed(Some("   "), "0.1.0", None)]
    #[case::malformed_override_fails_closed(Some("v1.2"), "0.1.0", None)]
    fn resolves_build_version_in_strict_order(
        #[case] override_val: Option<&str>,
        #[case] pkg_version: &str,
        #[case] expected: Option<&str>,
    ) {
        let res = resolve_build_version(override_val, pkg_version);
        match expected {
            Some(exp) => assert_eq!(res.expect("should resolve"), exp),
            None => assert!(res.is_err(), "expected malformed override to fail"),
        }
    }
}
